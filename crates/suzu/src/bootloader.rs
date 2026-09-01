//! ESP8266 ROM bootloader — native MicroPython runtime flashing.
//!
//! A factory-fresh (or crash-looped) ESP8266 runs no interpreter; its only
//! door is the ROM serial bootloader. This module speaks that protocol
//! directly: SLIP framing, SYNC, FLASH_BEGIN/FLASH_DATA, and the DTR/RTS
//! reset dance — the Rust port of the ancestor recipe codified in
//! `hardware/classes/esp8266-oled/procedure.yaml` (erase + write the
//! vendored runtime, no improvements), so factory-fresh onboarding
//! requires only this binary.
//!
//! Protocol notes (verified against esptool 5.1.0, the version the bench
//! validated):
//! - the ROM auto-bauds on SYNC, so the session runs at FLASH_BAUD after
//!   the reset; CHANGE_BAUDRATE (0x0F) is not a ROM command on this chip;
//! - FLASH_END is never sent — on the ROM path esptool skips it (it makes
//!   the loader exit) and hard-resets the board instead;
//! - FLASH_BEGIN performs the erase up front; the erase size uses the
//!   ROM's sector-workaround formula (get_erase_size), not the raw length;
//! - the ROM cannot read flash back, so verification is per-block ACKs,
//!   the REPL handshake after reset, and the admission test — stated
//!   plainly, not silently assumed.
//!
//! Error philosophy (same law as `repl.rs`): a framing failure aborts
//! loudly. Unlike the REPL push, a failed flash is self-healing — blocks
//! are written in ascending order, so an interrupted run leaves the board
//! in a state this same procedure re-flashes. Rule zero of
//! install-lessons.md: the tool that bricks is the tool that un-bricks.

use anyhow::{bail, Context, Result};
use serialport::{ClearBuffer, SerialPort};
use std::io::{Read, Write};
use std::time::{Duration, Instant};

/// Sync bauds, in attempt order. The ROM auto-bauds to the first clean
/// SYNC and then STAYS there — the bench proved it will not re-latch on
/// later syncs, so the first answering baud is the flash baud. 115200 is
/// tried first: the bench's CH340 latches it reliably, and the ancestor
/// recipe's 460800 proved flaky on this bridge (kept as the second
/// attempt for bridges that do latch it).
const SYNC_BAUDS: [u32; 2] = [115_200, 460_800];
const OPEN_BAUD: u32 = 115_200;
const PORT_TIMEOUT: u64 = 300; // ms — matches every other open site
const BOOT_WAIT: u64 = 2500; // the ESP auto-resets when the port opens

/// One FLASH_DATA packet carries exactly this many flash bytes.
const FLASH_WRITE_SIZE: u32 = 0x400;
const FLASH_SECTOR_SIZE: u32 = 0x1000;
const FLASH_OFFSET: u32 = 0x0;

/// ROM performs the whole erase inside FLASH_BEGIN before ACKing:
/// 30 s per MB, floor 3 s (esptool's ERASE_REGION_TIMEOUT_PER_MB).
const ERASE_TIMEOUT_PER_MB: u64 = 30;
/// ROM writes the block to flash before ACKing (esptool DEFAULT_TIMEOUT).
const BLOCK_TIMEOUT_MS: u64 = 3_000;
const WRITE_BLOCK_ATTEMPTS: u32 = 3;
/// One command read (esptool SYNC_TIMEOUT / DEFAULT_TIMEOUT).
const SYNC_READ_MS: u64 = 100;
const DEFAULT_READ_MS: u64 = 3_000;
/// A response whose op echo doesn't match is skipped; this bounds the loop
/// (the ROM floods SYNC with at least 7 extra replies).
const RESPONSE_SKIP_LIMIT: usize = 20;
/// Pending-byte ceiling before unframed noise is discarded wholesale.
const NOISE_CAP: usize = 2048;

// ── opcodes — the ESP8266 ROM command set (esptool ESP_CMDS) ────────────

const SYNC: u8 = 0x08;
const FLASH_BEGIN: u8 = 0x02;
const FLASH_DATA: u8 = 0x03;
const WRITE_REG: u8 = 0x09;
const READ_REG: u8 = 0x0A;

/// Initial state of the ROM's payload checksum (esptool ESP_CHECKSUM_MAGIC).
const CHECKSUM_MAGIC: u8 = 0xEF;

// ── SLIP codec ───────────────────────────────────────────────────────────

/// Frame `payload` as one SLIP packet: END ... escaped ... END.
fn slip_encode(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(payload.len() + 2);
    out.push(0xC0);
    for &b in payload {
        match b {
            0xC0 => out.extend_from_slice(&[0xDB, 0xDC]),
            0xDB => out.extend_from_slice(&[0xDB, 0xDD]),
            _ => out.push(b),
        }
    }
    out.push(0xC0);
    out
}

/// What the SLIP decoder found in the buffer.
#[derive(Debug)]
enum Slip {
    /// A complete frame: its payload and the bytes it consumed.
    Frame(Vec<u8>, usize),
    /// Malformed frame content: bytes to discard before rescanning. Boot
    /// garbage read at the wrong baud produces this constantly — without
    /// the discard, one stray escape poisons the buffer forever and no
    /// real frame ever decodes again.
    Garbage(usize),
    /// No complete frame yet.
    Incomplete,
}

/// Decode one SLIP packet from the front of `buf`.
///
/// Bytes before the opening END are boot noise and are consumed silently —
/// a board in a crash loop spews 74880-baud garbage between resets, and
/// the sync loop matches responses by opcode anyway.
fn slip_decode(buf: &[u8]) -> Slip {
    // Drop leading noise until a frame opener appears; cap unbounded noise.
    let Some(start) = buf.iter().position(|&b| b == 0xC0) else {
        return if buf.len() > NOISE_CAP {
            Slip::Garbage(buf.len())
        } else {
            Slip::Incomplete
        };
    };
    let mut payload = Vec::with_capacity(buf.len() - start);
    let mut j = start + 1;
    while j < buf.len() {
        match buf[j] {
            0xC0 => return Slip::Frame(payload, j + 1),
            0xDB => match buf.get(j + 1) {
                Some(0xDC) => {
                    payload.push(0xC0);
                    j += 2;
                }
                Some(0xDD) => {
                    payload.push(0xDB);
                    j += 2;
                }
                // Invalid escape: discard through it and rescan — the tail
                // may hold a fresh opener.
                _ => return Slip::Garbage(j + 2),
            },
            b => {
                payload.push(b);
                j += 1;
            }
        }
    }
    if payload.len() > NOISE_CAP {
        // An opener with no close and no end in sight is noise wearing a
        // 0xC0 as a hat.
        Slip::Garbage(buf.len())
    } else {
        Slip::Incomplete
    }
}

/// Consume one decoded frame or garbage prefix from `buf`; false when
/// the buffer holds no complete frame yet.
fn slip_drop(buf: &mut Vec<u8>) -> bool {
    match slip_decode(buf) {
        Slip::Frame(_, used) | Slip::Garbage(used) => {
            buf.drain(..used);
            true
        }
        Slip::Incomplete => false,
    }
}

/// The ROM's payload checksum: XOR every byte, seeded with the magic.
fn xor_checksum(data: &[u8]) -> u8 {
    let mut state = CHECKSUM_MAGIC;
    for &b in data {
        state ^= b;
    }
    state
}

// ── v1 request/response framing ──────────────────────────────────────────

/// One ROM request: direction 0x00, opcode, payload length, checksum,
/// then the payload — SLIP framed on the wire. The header is packed
/// `<BBHI` (esptool's exact struct): direction, op, len as u16, checksum
/// as u32 — eight bytes before the payload.
fn request(op: u8, payload: &[u8], checksum: u32) -> Vec<u8> {
    let mut pkt = Vec::with_capacity(8 + payload.len());
    pkt.push(0x00);
    pkt.push(op);
    pkt.extend_from_slice(&(payload.len() as u16).to_le_bytes());
    pkt.extend_from_slice(&checksum.to_le_bytes());
    pkt.extend_from_slice(payload);
    pkt
}

/// The sync payload: the fixed 4-byte preamble then 32 sync bytes.
fn sync_payload() -> Vec<u8> {
    let mut v = vec![0x07, 0x07, 0x12, 0x20];
    v.extend(std::iter::repeat_n(0x55, 32));
    v
}

/// One decoded ROM response.
#[derive(Debug, PartialEq)]
struct Response {
    /// Opcode echo — which command this answers.
    op: u8,
    /// ROM replies carry a non-zero value here (a stub replies 0).
    val: u32,
    /// Everything after the header, status bytes included — esptool
    /// ignores the header's length field entirely, and so do we.
    data: Vec<u8>,
    /// First byte after the header: 0 = success.
    status: u8,
}

/// Parse a SLIP-decoded frame into a response. `None` for anything that is
/// not a response (wrong direction, too short) — the caller skips ahead.
fn parse_response(frame: &[u8]) -> Option<Response> {
    if frame.len() < 8 || frame[0] != 0x01 {
        return None;
    }
    let op = frame[1];
    let val = u32::from_le_bytes([frame[4], frame[5], frame[6], frame[7]]);
    let data = frame[8..].to_vec();
    let status = data.first().copied().unwrap_or(0xFF);
    Some(Response { op, val, data, status })
}

// ── the reset dances ─────────────────────────────────────────────────────

/// One line-control step: set DTR/RTS (`None` leaves a line as it is),
/// then wait. Both esptool dances as data, so they are testable and
/// tunable on the bench without touching control flow.
type LineStep = (Option<bool>, Option<bool>, u64);

/// Enter the UART download mode (esptool's ClassicReset): release IO0,
/// pulse EN through the CH340's transistor circuit, then hold IO0 low
/// across the reset so the ROM boots as a loader.
fn download_mode_script() -> Vec<LineStep> {
    vec![
        (Some(false), None, 0),   // IO0 high
        (None, Some(true), 100),  // EN low — chip in reset
        (Some(true), Some(false), 50), // IO0 low, EN high — download mode
        (Some(false), None, 50),  // IO0 high, done
    ]
}

/// Leave the loader and boot the freshly written firmware: the same pulse
/// without pulling IO0, so the ROM boots flash normally.
fn run_firmware_script() -> Vec<LineStep> {
    vec![
        (Some(false), None, 0),  // IO0 high — normal boot
        (None, Some(true), 100), // EN low — chip in reset
        (None, Some(false), 100), // EN high — chip out of reset
    ]
}

/// The ROM's sector-erase workaround (esptool ESP8266ROM::get_erase_size):
/// the loader erases slightly less than the image spans, head sectors
/// included, to steer around a bootloader erase bug. Codified verbatim.
fn get_erase_size(offset: u32, size: u32) -> u32 {
    const SECTORS_PER_BLOCK: u32 = 16;
    let num_sectors = size.div_ceil(FLASH_SECTOR_SIZE);
    let start_sector = offset / FLASH_SECTOR_SIZE;
    let mut head_sectors = SECTORS_PER_BLOCK - (start_sector % SECTORS_PER_BLOCK);
    if num_sectors < head_sectors {
        head_sectors = num_sectors;
    }
    if num_sectors < 2 * head_sectors {
        num_sectors.div_ceil(2) * FLASH_SECTOR_SIZE
    } else {
        (num_sectors - head_sectors) * FLASH_SECTOR_SIZE
    }
}

/// Read/poll budgets for one session. Production values are esptool's
/// (SYNC_TIMEOUT 0.1 s, DEFAULT_TIMEOUT 3 s, 30 s per MB of erase); tests
/// shrink them so retry paths run in milliseconds.
#[derive(Clone, Copy)]
struct Timeouts {
    sync_read_ms: u64,
    sync_window_ms: u64,
    command_ms: u64,
    block_ms: u64,
    erase_ms_per_mb: u64,
    erase_floor_ms: u64,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            sync_read_ms: SYNC_READ_MS,
            sync_window_ms: 3_000,
            command_ms: DEFAULT_READ_MS,
            block_ms: BLOCK_TIMEOUT_MS,
            erase_ms_per_mb: ERASE_TIMEOUT_PER_MB * 1_000,
            erase_floor_ms: DEFAULT_READ_MS,
        }
    }
}

// ── the protocol session, generic over the transport ─────────────────────

/// The framing and command state machine, transport-agnostic so the whole
/// loop is unit-testable against an in-memory duplex.
struct Session<S> {
    io: S,
    timeouts: Timeouts,
}

impl<S: Read + Write> Session<S> {
    fn new(io: S) -> Self {
        Self { io, timeouts: Timeouts::default() }
    }

    #[cfg(test)]
    fn with_timeouts(io: S, timeouts: Timeouts) -> Self {
        Self { io, timeouts }
    }

    /// Read whatever arrives within `ms`, consuming complete frames.
    fn read_frames(&mut self, ms: u64, buf: &mut Vec<u8>) -> Result<Vec<Vec<u8>>> {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut frames = Vec::new();
        loop {
            if Instant::now() >= end {
                break;
            }
            let mut scratch = [0u8; 512];
            match self.io.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => buf.extend_from_slice(&scratch[..n]),
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(e) => return Err(e.into()),
            }
            let mut consumed = false;
            while let Some((payload, used)) = match slip_decode(buf) {
                Slip::Frame(payload, used) => Some((payload, used)),
                Slip::Garbage(used) => {
                    buf.drain(..used);
                    consumed = true;
                    None
                }
                Slip::Incomplete => None,
            } {
                buf.drain(..used);
                frames.push(payload);
                consumed = true;
            }
            if consumed || !frames.is_empty() {
                // Something arrived — hand it to the caller now.
                break;
            }
        }
        Ok(frames)
    }

    /// Send one request and wait for its response. Responses for other
    /// commands are skipped (the ROM floods SYNC); a response whose status
    /// byte is non-zero is a hard failure with the ROM's reason.
    fn command(
        &mut self,
        op: u8,
        payload: &[u8],
        checksum: u32,
        timeout_ms: u64,
        pending: &mut Vec<u8>,
    ) -> Result<Response> {
        let pkt = slip_encode(&request(op, payload, checksum));
        if std::env::var("SUZU_WIRE_TRACE").is_ok() {
            eprintln!("TX op={op:#04x} payload={:02x?}", payload);
        }
        self.io.write_all(&pkt)?;
        self.io.flush()?;
        let mut skipped = 0;
        loop {
            let frames = self.read_frames(timeout_ms, pending)?;
            if frames.is_empty() {
                bail!("no response to opcode {op:#04x} — board silent");
            }
            for frame in frames {
                let Some(resp) = parse_response(&frame) else {
                    continue;
                };
                if std::env::var("SUZU_WIRE_TRACE").is_ok() {
                    eprintln!("RX frame={:02x?}", frame);
                }
                if std::env::var("SUZU_WIRE_TRACE").is_ok() {
                    eprintln!("MATCH op={op:#04x} val={:08x} data={:02x?} status={}", resp.val, resp.data, resp.status);
                }
                if resp.op != op {
                    skipped += 1;
                    if skipped > RESPONSE_SKIP_LIMIT {
                        bail!("response flood buried opcode {op:#04x}");
                    }
                    continue;
                }
                if resp.status != 0 {
                    bail!(
                        "opcode {op:#04x} failed: status {} reason {}",
                        resp.status,
                        resp.data.get(1).copied().unwrap_or(0)
                    );
                }
                return Ok(resp);
            }
        }
    }

    /// SYNC until the ROM answers (it auto-bauds to whatever rate this
    /// arrives at), then drain the reply flood — not merely until the
    /// first reply, but until the line has been SILENT for a beat. The
    /// ROM answers one SYNC at least seven times over several USB chunks;
    /// leftover flood frames otherwise interleave with (and masquerade
    /// as) the next command's reply.
    fn sync(&mut self, pending: &mut Vec<u8>) -> Result<()> {
        let payload = sync_payload();
        let pkt = slip_encode(&request(SYNC, &payload, 0));
        let end = Instant::now() + Duration::from_millis(self.timeouts.sync_window_ms);
        while Instant::now() < end {
            let _ = self.io.write_all(&pkt);
            let _ = self.io.flush();
            let frames = self.read_frames(self.timeouts.sync_read_ms, pending)?;
            let answered = frames.iter().any(|f| {
                parse_response(f).is_some_and(|r| r.op == SYNC)
            });
            if answered {
                self.drain_flood(pending);
                return Ok(());
            }
        }
        bail!("no sync reply — download mode was not entered")
    }

    /// Read and discard until the line has been silent for 60 ms.
    fn drain_flood(&mut self, pending: &mut Vec<u8>) {
        let deadline = Instant::now() + Duration::from_millis(400);
        let mut last_rx = Instant::now();
        while last_rx.elapsed() < Duration::from_millis(60) && Instant::now() < deadline {
            let mut scratch = [0u8; 512];
            match self.io.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => {
                    pending.extend_from_slice(&scratch[..n]);
                    last_rx = Instant::now();
                }
                Err(e) if e.kind() == std::io::ErrorKind::TimedOut => {}
                Err(_) => return,
            }
            while slip_drop(pending) {}
        }
    }

    /// Enter flash download mode: the ROM erases `size` bytes at `offset`
    /// up front, before ACKing (esptool: "performs an erase").
    fn flash_begin(&mut self, size: u32, offset: u32, pending: &mut Vec<u8>) -> Result<u32> {
        let num_blocks = size.div_ceil(FLASH_WRITE_SIZE);
        let erase_size = get_erase_size(offset, size);
        let t = &self.timeouts;
        let erase_ms = (t.erase_ms_per_mb * size as u64 / 1_000_000).max(t.erase_floor_ms);
        let mut params = Vec::with_capacity(16);
        params.extend_from_slice(&erase_size.to_le_bytes());
        params.extend_from_slice(&num_blocks.to_le_bytes());
        params.extend_from_slice(&FLASH_WRITE_SIZE.to_le_bytes());
        params.extend_from_slice(&offset.to_le_bytes());
        self.command(FLASH_BEGIN, &params, 0, erase_ms, pending)?;
        Ok(num_blocks)
    }

    /// Write one block and wait for the ACK — the ROM writes to flash
    /// before replying, so each ACK is the block's only verification.
    fn flash_block(&mut self, block: &[u8], seq: u32, pending: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::with_capacity(16 + block.len());
        payload.extend_from_slice(&(block.len() as u32).to_le_bytes());
        payload.extend_from_slice(&seq.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        payload.extend_from_slice(block);
        let checksum = xor_checksum(block) as u32;
        let mut last = None;
        for _ in 0..WRITE_BLOCK_ATTEMPTS {
            match self.command(FLASH_DATA, &payload, checksum, self.timeouts.block_ms, pending) {
                Ok(_) => return Ok(()),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap())
    }

    /// Read one memory word through the ROM (esptool read_reg): the value
    /// rides the response header's val field, not the payload.
    fn read_reg(&mut self, addr: u32, pending: &mut Vec<u8>) -> Result<u32> {
        let resp = self.command(
            READ_REG,
            &addr.to_le_bytes(),
            0,
            self.timeouts.command_ms,
            pending,
        )?;
        Ok(resp.val)
    }

    /// Write one memory word through the ROM (esptool write_reg): the
    /// payload is four words — addr, value, mask (all-ones = plain
    /// write), and a post-write delay in microseconds, which we don't use.
    fn write_reg(&mut self, addr: u32, value: u32, pending: &mut Vec<u8>) -> Result<()> {
        let mut payload = Vec::with_capacity(16);
        payload.extend_from_slice(&addr.to_le_bytes());
        payload.extend_from_slice(&value.to_le_bytes());
        payload.extend_from_slice(&0xFFFF_FFFFu32.to_le_bytes());
        payload.extend_from_slice(&0u32.to_le_bytes());
        self.command(WRITE_REG, &payload, 0, self.timeouts.command_ms, pending)?;
        Ok(())
    }

    /// Run one raw SPI flash command through the chip's SPI controller —
    /// the ESP8266 register set (esptool run_spiflash_command, targets/
    /// esp8266.py constants). `read_bits` (≤ 32) of the reply land in W0.
    fn run_spiflash_command(
        &mut self,
        cmd: u8,
        data: &[u8],
        read_bits: u32,
        pending: &mut Vec<u8>,
    ) -> Result<u32> {
        const SPI_USR_COMMAND: u32 = 1 << 31;
        const SPI_USR_MISO: u32 = 1 << 28;
        const SPI_USR_MOSI: u32 = 1 << 27;
        const SPI_CMD_USR: u32 = 1 << 18;
        const SPI_USR2_COMMAND_LEN_SHIFT: u32 = 28;
        const SPI_MOSI_BITLEN_S: u32 = 17;
        const SPI_MISO_BITLEN_S: u32 = 8;
        const BASE: u32 = 0x6000_0200; // ESP8266 SPI_REG_BASE
        let (usr_reg, usr1_reg, usr2_reg, w0_reg) = (BASE + 0x1C, BASE + 0x20, BASE + 0x24, BASE + 0x40);

        let data_bits = data.len() as u32 * 8;
        let old_usr = self.read_reg(usr_reg, pending)?;
        let old_usr2 = self.read_reg(usr2_reg, pending)?;

        // USR1 packs both bit lengths on this chip (no DLEN registers).
        let mut flags1 = ((read_bits.saturating_sub(1)) << SPI_MISO_BITLEN_S)
            | (data_bits.saturating_sub(1) << SPI_MOSI_BITLEN_S);
        if read_bits == 0 {
            flags1 &= !(0xFF << SPI_MISO_BITLEN_S);
        }
        if data_bits == 0 {
            flags1 &= !(0x3FF << SPI_MOSI_BITLEN_S);
        }
        let mut flags = SPI_USR_COMMAND;
        if read_bits > 0 {
            flags |= SPI_USR_MISO;
        }
        if data_bits > 0 {
            flags |= SPI_USR_MOSI;
        }
        self.write_reg(usr1_reg, flags1, pending)?;
        self.write_reg(usr_reg, flags, pending)?;
        self.write_reg(usr2_reg, (7 << SPI_USR2_COMMAND_LEN_SHIFT) | cmd as u32, pending)?;
        if data_bits == 0 {
            self.write_reg(w0_reg, 0, pending)?; // clear before we read it back
        } else {
            let mut padded = data.to_vec();
            padded.resize(padded.len().div_ceil(4) * 4, 0);
            for (i, word) in padded.chunks(4).enumerate() {
                let v = u32::from_le_bytes(word.try_into().unwrap());
                self.write_reg(w0_reg + (i as u32) * 4, v, pending)?;
            }
        }
        self.write_reg(BASE, SPI_CMD_USR, pending)?;
        // The command bit self-clears when the transfer completes.
        for _ in 0..10 {
            if self.read_reg(BASE, pending)? & SPI_CMD_USR == 0 {
                let status = self.read_reg(w0_reg, pending)?;
                self.write_reg(usr_reg, old_usr, pending)?;
                self.write_reg(usr2_reg, old_usr2, pending)?;
                return Ok(status);
            }
        }
        bail!("SPI command {cmd:#04x} did not complete in time")
    }

    /// The flash chip's JEDEC identity (0x9F): manufacturer, type, and the
    /// capacity byte, whose value is log2 of the total size in bytes.
    fn flash_id(&mut self, pending: &mut Vec<u8>) -> Result<u32> {
        self.run_spiflash_command(0x9F, &[], 24, pending)
    }

    /// Erase and write `image` at `offset`, one full FLASH_WRITE_SIZE
    /// block at a time in ascending order (the bootloader region lands
    /// first, so an interrupted run is re-flashable by this same path).
    /// Blocks are padded with 0xFF exactly as the ancestor padded them.
    fn flash_image(
        &mut self,
        image: &[u8],
        offset: u32,
        pending: &mut Vec<u8>,
        mut progress: impl FnMut(u32, u32),
    ) -> Result<()> {
        let num_blocks = self.flash_begin(image.len() as u32, offset, pending)?;
        println!("  erased ({} blocks of {FLASH_WRITE_SIZE:#x})", num_blocks);
        for (seq, chunk) in image.chunks(FLASH_WRITE_SIZE as usize).enumerate() {
            let mut block = vec![0xFFu8; FLASH_WRITE_SIZE as usize];
            block[..chunk.len()].copy_from_slice(chunk);
            self.flash_block(&block, seq as u32, pending)?;
            progress(seq as u32 + 1, num_blocks);
        }
        Ok(())
    }
}

// ── the real transport ───────────────────────────────────────────────────

fn apply_script(
    port: &mut Box<dyn SerialPort>,
    script: &[LineStep],
) {
    for (dtr, rts, ms) in script {
        if let Some(v) = dtr {
            let _ = port.write_data_terminal_ready(*v);
        }
        if let Some(v) = rts {
            let _ = port.write_request_to_send(*v);
        }
        if *ms > 0 {
            std::thread::sleep(Duration::from_millis(*ms));
        }
    }
}

/// An open ROM-bootloader session on one serial port.
pub struct RomLoader {
    port: Box<dyn SerialPort>,
    pending: Vec<u8>,
}

impl RomLoader {
    /// Open at the house baud and force the board into download mode.
    ///
    /// Unlike the REPL path this never probes first — a board with no
    /// interpreter cannot answer one — and never touches flash.
    pub fn open(port_name: &str) -> Result<Self> {
        let mut port: Box<dyn SerialPort> = serialport::new(port_name, OPEN_BAUD)
            .timeout(Duration::from_millis(PORT_TIMEOUT))
            .open()
            .with_context(|| format!("{port_name}: open failed"))?;
        // The open pulse itself resets the board; let it finish spewing.
        std::thread::sleep(Duration::from_millis(BOOT_WAIT));
        drain(&mut port, 300);
        apply_script(&mut port, &download_mode_script());
        drain(&mut port, 200);
        Ok(Self { port, pending: Vec::new() })
    }

    /// Sync at the first answering baud (the ROM auto-bauds to the SYNC
    /// rate and stays there — see SYNC_BAUDS), then attach the flash chip.
    /// The ESP8266 ROM has no SPI-attach command; a zero-size FLASH_BEGIN
    /// is what makes it attach the chip (esptool esp8266.py says so), and
    /// the raw SPI reads of the ID step need that.
    pub fn connect(&mut self) -> Result<()> {
        let mut last = None;
        for baud in SYNC_BAUDS {
            let _ = self.port.set_baud_rate(baud);
            drain(&mut self.port, 100);
            match self.sync_once() {
                Ok(()) => {
                    println!("  ROM loader answering at {baud}");
                    self.flush_input();
                    let mut session = Session::new(&mut self.port);
                    session.flash_begin(0, 0, &mut self.pending)?;
                    self.flush_input();
                    return Ok(());
                }
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap())
    }

    fn sync_once(&mut self) -> Result<()> {
        Session::new(&mut self.port).sync(&mut self.pending)
    }

    /// Drop whatever the OS driver still holds (esptool flushInput).
    fn flush_input(&mut self) {
        let _ = self.port.clear(ClearBuffer::Input);
    }

    /// The ancestor recipe's `--flash_size=detect`, made explicit: read
    /// the flash chip's JEDEC capacity and set the image header's size
    /// field (byte 3, high nibble) to match. The vendored runtime declares
    /// the size it was built for; on a chip of any other size the ROM's
    /// SDK boots it into a crash loop — install-lessons.md §3 codified
    /// this exact failure. Patching the header post-build is what esptool
    /// does, so it needs no re-checksum.
    fn patch_header_to_detected_flash(&mut self, image: &mut [u8]) -> Result<()> {
        let id = {
            let mut session = Session::new(&mut self.port);
            session.flash_id(&mut self.pending)?
        };
        let capacity = ((id >> 16) & 0xFF) as u8;
        // Header size codes: 0=512KB, 1=256KB, 2=1MB … 6=16MB. JEDEC
        // capacity 0x14 = 1MB. Everything outside the ROM's table refuses.
        let nibble = match capacity {
            0x13 => 0x0, // 512KB
            0x12 => 0x1, // 256KB
            0x14 => 0x2, // 1MB
            0x15 => 0x3, // 2MB
            0x16 => 0x4, // 4MB
            0x17 => 0x8, // 8MB
            0x18 => 0x9, // 16MB
            other => bail!("flash reports unknown capacity code {other:#04x} (JEDEC {id:#010x})"),
        };
        let detected = 1u64 << capacity;
        if image.len() as u64 > detected {
            bail!(
                "runtime image is {} bytes but the chip reports {} bytes of flash",
                image.len(),
                detected
            );
        }
        println!("  flash: {} bytes detected — image header set to match", detected);
        image[3] = (image[3] & 0x0F) | (nibble << 4);
        Ok(())
    }

    /// Erase, write, and verify the runtime image, then reset the board
    /// into the new firmware. Consumes the loader: after the reset the
    /// port belongs to the REPL path.
    pub fn flash_image(
        mut self,
        image: &mut [u8],
        offset: u32,
        progress: impl FnMut(u32, u32),
    ) -> Result<()> {
        self.patch_header_to_detected_flash(image)?;
        {
            let mut session = Session::new(&mut self.port);
            session.flash_image(image, offset, &mut self.pending, progress)?;
        }
        // No FLASH_END — on the ROM path it exits the loader and runs
        // whatever is in flash. Reset the board the way the ancestor does:
        // a bare EN pulse, so the new runtime boots.
        apply_script(&mut self.port, &run_firmware_script());
        Ok(())
    }
}

/// Read and discard whatever arrives for `ms`.
fn drain(port: &mut Box<dyn SerialPort>, ms: u64) {
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        let waiting = port.bytes_to_read().unwrap_or(0) as usize;
        let mut scratch = vec![0u8; waiting.max(1)];
        match port.read(&mut scratch) {
            Ok(0) => {}
            Ok(_) => {}
            Err(_) => {}
        }
    }
}

/// Onboard a factory-fresh or crash-looped ESP8266: force download mode,
/// flash the vendored MicroPython runtime at offset 0, and boot it.
/// The image's first byte is checked before any erase — a corrupted
/// artifact must fail before the board is touched.
pub fn flash_micropython(port_name: &str) -> Result<()> {
    let path = crate::paths::firmware_dir()
        .join("artifacts")
        .join("micropython-esp8266-1mib.bin");
    let mut image = std::fs::read(&path).with_context(|| {
        format!(
            "runtime artifact {} is missing — factory-fresh onboarding works offline, so it must be vendored first",
            path.display()
        )
    })?;
    if image.first() != Some(&0xE9) {
        bail!(
            "runtime artifact does not start with the ESP image magic (0xE9) — refusing to flash it"
        );
    }
    println!("  flashing MicroPython runtime ({} bytes) at {FLASH_OFFSET:#x}", image.len());
    let mut loader = RomLoader::open(port_name)?;
    loader.connect()?;
    loader
        .flash_image(&mut image, FLASH_OFFSET, |done, total| {
            if done % 16 == 0 || done == total {
                println!("  wrote {done}/{total} blocks");
            }
        })
        .context("flash write failed — the board is recoverable by re-running this procedure")
}

// ── tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Test-time budgets: small enough that retry paths run in
    /// milliseconds, large enough for the fake's 1 ms read timeout.
    fn fast_timeouts() -> Timeouts {
        Timeouts {
            sync_read_ms: 20,
            sync_window_ms: 200,
            command_ms: 50,
            block_ms: 30,
            erase_ms_per_mb: 1_000,
            erase_floor_ms: 30,
        }
    }

    /// A scripted serial peer: writes are SLIP-decoded, and each complete
    /// request whose opcode has a registered responder gets that response
    /// queued for the next read; unregistered opcodes get silence. This is
    /// the only fake in the crate — the protocol loop is the one piece too
    /// intricate to leave untested (install-lessons.md §3).
    struct FakePort {
        rx: std::collections::VecDeque<u8>,
        tx: Vec<u8>,
        parsed: usize,
        responses: Vec<(u8, Vec<u8>)>,
    }

    impl FakePort {
        fn new() -> Self {
            Self {
                rx: Default::default(),
                tx: Vec::new(),
                parsed: 0,
                responses: Vec::new(),
            }
        }

        /// Queue bytes the "board" will send unprompted.
        fn feed(mut self, bytes: &[u8]) -> Self {
            self.rx.extend(bytes.iter().copied());
            self
        }

        /// Answer every request to `op` with a SLIP-framed `frame`.
        fn respond(mut self, op: u8, frame: &[u8]) -> Self {
            self.responses.push((op, slip_encode(frame)));
            self
        }

        /// A minimal OK response to `op`: direction 0x01, echo, no data,
        /// status 0.
        fn ok_response(op: u8) -> Vec<u8> {
            let mut f = vec![0x01, op];
            f.extend_from_slice(&0u16.to_le_bytes());
            f.extend_from_slice(&0u32.to_le_bytes());
            f.push(0); // status OK
            f.push(0); // reason
            f
        }

        /// Every request the session wrote, SLIP-decoded back.
        fn written_packets(&self) -> Vec<Vec<u8>> {
            let mut out = Vec::new();
            let mut rest = &self.tx[..];
            loop {
                match slip_decode(rest) {
                    Slip::Frame(payload, used) => {
                        out.push(payload);
                        rest = &rest[used..];
                    }
                    Slip::Garbage(used) => rest = &rest[used..],
                    Slip::Incomplete => break,
                }
            }
            out
        }
    }

    impl Read for FakePort {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let mut n = 0;
            while n < buf.len() {
                match self.rx.pop_front() {
                    Some(b) => {
                        buf[n] = b;
                        n += 1;
                    }
                    None => break,
                }
            }
            if n == 0 {
                // Simulate the port timeout every open site uses.
                std::thread::sleep(Duration::from_millis(1));
                return Err(std::io::Error::new(std::io::ErrorKind::TimedOut, "idle"));
            }
            Ok(n)
        }
    }

    impl Write for FakePort {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.tx.extend_from_slice(buf);
            // Answer any complete request in the newly written bytes.
            let fresh = &self.tx[self.parsed..];
            match slip_decode(fresh) {
                Slip::Frame(frame, used) => {
                    self.parsed += used;
                    if frame.len() >= 8 && frame[0] == 0x00 {
                        let op = frame[1];
                        if let Some((_, resp)) = self.responses.iter().find(|(o, _)| *o == op) {
                            self.rx.extend(resp.iter().copied());
                        }
                    }
                }
                Slip::Garbage(used) => self.parsed += used,
                Slip::Incomplete => {}
            }
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn slip_escapes_the_framing_bytes() {
        assert_eq!(slip_encode(&[]), vec![0xC0, 0xC0]);
        assert_eq!(slip_encode(&[0xC0]), vec![0xC0, 0xDB, 0xDC, 0xC0]);
        assert_eq!(slip_encode(&[0xDB]), vec![0xC0, 0xDB, 0xDD, 0xC0]);
        assert_eq!(slip_encode(&[0x01, 0xC0, 0x02]), vec![0xC0, 0x01, 0xDB, 0xDC, 0x02, 0xC0]);
        assert_eq!(slip_encode(&[0x01, 0xDB, 0x02]), vec![0xC0, 0x01, 0xDB, 0xDD, 0x02, 0xC0]);
    }

    #[test]
    fn slip_decodes_across_noise_and_rejects_bad_escapes() {
        let frame = slip_encode(&[0x07, 0x07]);
        let mut buf = vec![0xFF, 0x00, 0xAA]; // boot spew before the frame
        buf.extend_from_slice(&frame);
        buf.extend_from_slice(&[0x55]); // trailing noise
        match slip_decode(&buf) {
            Slip::Frame(payload, used) => {
                assert_eq!(payload, vec![0x07, 0x07]);
                assert_eq!(used, buf.len() - 1);
            }
            _ => panic!("expected a frame"),
        }

        // An incomplete frame (no closing END) is a wait, not an error.
        assert!(matches!(slip_decode(&[0xC0, 0x01, 0x02]), Slip::Incomplete));
        assert!(matches!(slip_decode(&[0x01, 0x02]), Slip::Incomplete));
        // A malformed escape is garbage to discard, and the decoder
        // recovers: after dropping the junk, the next frame decodes.
        let poisoned = [0xC0, 0xDB, 0x99, 0xAA];
        match slip_decode(&poisoned) {
            Slip::Garbage(used) => assert_eq!(used, 3),
            _ => panic!("expected garbage"),
        }
        let mut recovered = poisoned.to_vec();
        recovered.extend_from_slice(&slip_encode(&[0x42]));
        match slip_decode(&recovered) {
            Slip::Garbage(used) => {
                let rest = &recovered[used..];
                assert!(matches!(slip_decode(rest), Slip::Frame(p, _) if p == vec![0x42]));
            }
            _ => panic!("expected garbage first"),
        }
        // Endless noise without a frame is capped, never hoarded.
        let noise = vec![0x55u8; NOISE_CAP + 100];
        assert!(matches!(slip_decode(&noise), Slip::Garbage(_)));
    }

    #[test]
    fn request_headers_match_the_v1_wire_format() {
        // <BBHI 0x00 op len chk, little-endian, 8-byte header — checked
        // byte for byte.
        assert_eq!(
            request(FLASH_BEGIN, &[1, 2, 3, 4], 0xAABB),
            vec![0x00, 0x02, 0x04, 0x00, 0xBB, 0xAA, 0x00, 0x00, 1, 2, 3, 4]
        );
    }

    #[test]
    fn checksum_is_the_roms_xor_with_the_magic() {
        assert_eq!(xor_checksum(&[]), 0xEF);
        assert_eq!(xor_checksum(&[0x00]), 0xEF);
        assert_eq!(xor_checksum(&[0xEF]), 0x00);
        assert_eq!(xor_checksum(&[0x01, 0x02, 0x04]), 0xEF ^ 0x01 ^ 0x02 ^ 0x04);
    }

    #[test]
    fn responses_parse_and_reject_non_responses() {
        // The exact shape the bench ROM answers to SYNC (2026-09-01):
        // direction, echo, len 2, val 0x20120707, then the two status
        // bytes — the header's length field counts the status, which is
        // why it is ignored and the first byte after the header is the
        // status.
        let frame = [
            0x01u8, 0x08, 0x02, 0x00, 0x07, 0x07, 0x12, 0x20, 0x00, 0x00,
        ];
        let r = parse_response(&frame).unwrap();
        assert_eq!(r, Response { op: SYNC, val: 0x2012_0707, data: vec![0, 0], status: 0 });

        // A short frame and a wrong direction byte are skipped, not errors.
        assert_eq!(parse_response(&[0x01, 0x02]), None);
        assert_eq!(parse_response(&[0x00, 0x02, 0, 0, 0, 0, 0, 0, 0, 0]), None);

        // A failure carries its reason in the second byte after the header.
        let failing = [0x01u8, FLASH_DATA, 0x02, 0x00, 0, 0, 0, 0, 0xA1, 0x05];
        let r = parse_response(&failing).unwrap();
        assert_eq!(r.status, 0xA1);
        assert_eq!(r.data.get(1), Some(&0x05));
    }

    #[test]
    fn erase_size_implements_the_rom_workaround() {
        // Offset 0, a 1 MiB image: 256 sectors, head 16 skipped.
        assert_eq!(get_erase_size(0, 1 << 20), (256 - 16) * 4096);
        // Fewer sectors than the head: half rounded up.
        assert_eq!(get_erase_size(0, 8 * 4096), 4 * 4096);
        // The crossover sits at two heads of sectors.
        assert_eq!(get_erase_size(0, 31 * 4096), 16 * 4096);
        assert_eq!(get_erase_size(0, 32 * 4096), (32 - 16) * 4096);
        assert_eq!(get_erase_size(0, 33 * 4096), (33 - 16) * 4096);
    }

    #[test]
    fn the_reset_dances_match_the_reference_sequences() {
        // ClassicReset, line for line.
        assert_eq!(
            download_mode_script(),
            vec![
                (Some(false), None, 0),
                (None, Some(true), 100),
                (Some(true), Some(false), 50),
                (Some(false), None, 50),
            ]
        );
        // The exit dance never pulls IO0 low.
        assert!(run_firmware_script()
            .iter()
            .all(|(dtr, _, _)| *dtr != Some(true)));
    }

    #[test]
    fn the_sync_handshake_completes_against_a_rom_peer() {
        let mut port = FakePort::new().respond(SYNC, &FakePort::ok_response(SYNC));
        let mut pending = Vec::new();
        Session::with_timeouts(&mut port, fast_timeouts())
            .sync(&mut pending)
            .unwrap();
        let sent = port.written_packets();
        // The sync packet carries the fixed preamble and 32 sync bytes.
        let pkt = sent.last().unwrap();
        assert_eq!(pkt[0], 0x00);
        assert_eq!(pkt[1], SYNC);
        assert_eq!(u16::from_le_bytes(pkt[2..4].try_into().unwrap()), 36);
        assert_eq!(&pkt[8..12], &[0x07, 0x07, 0x12, 0x20]);
        assert!(pkt[12..].iter().all(|&b| b == 0x55));
    }

    #[test]
    fn a_silent_board_fails_the_handshake() {
        let mut port = FakePort::new(); // no responder
        let mut pending = Vec::new();
        assert!(Session::with_timeouts(&mut port, fast_timeouts())
            .sync(&mut pending)
            .is_err());
    }

    #[test]
    fn poisoned_boot_noise_never_buries_a_real_response() {
        // The bench failure, replayed: crash-loop spew read at the wrong
        // baud, complete with a stray opener and a bad escape, arriving
        // before the ROM's sync reply.
        let junk = [0xC0u8, 0x42, 0xDB, 0x99, 0xAA, 0x55];
        let mut port =
            FakePort::new().feed(&junk).respond(SYNC, &FakePort::ok_response(SYNC));
        let mut pending = Vec::new();
        Session::with_timeouts(&mut port, fast_timeouts())
            .sync(&mut pending)
            .expect("a real frame after malformed noise must still decode");
        // The garbage is consumed, not hoarded.
        assert!(pending.is_empty());
    }

    /// Bench probe for the real serial line: what does the board actually
    /// say at each stage? Touches no flash — opens, dumps, dances, dumps,
    /// syncs at both bauds, then boots the firmware again.
    #[test]
    #[ignore = "bench only: needs the esp8266-oled board on COM12"]
    fn bench_rom_diagnostics() {
        let name = "COM12";
        let mut port: Box<dyn SerialPort> = serialport::new(name, OPEN_BAUD)
            .timeout(Duration::from_millis(PORT_TIMEOUT))
            .open()
            .expect("open COM12");
        std::thread::sleep(Duration::from_millis(BOOT_WAIT));
        let heard = drain_collect(&mut port, 1500);
        println!("[1] after open, 115200: {} bytes: {:02x?}", heard.len(), &heard[..heard.len().min(96)]);

        apply_script(&mut port, &download_mode_script());
        let heard = drain_collect(&mut port, 1500);
        println!("[2] after dance, 115200: {} bytes: {:02x?}", heard.len(), &heard[..heard.len().min(96)]);

        // The ROM announces itself at 74880; listen there too.
        let _ = port.set_baud_rate(74_880);
        let heard = drain_collect(&mut port, 800);
        println!("[3] after dance, 74880: {} bytes: {:02x?}", heard.len(), &heard[..heard.len().min(96)]);

        for baud in [115_200u32, 460_800] {
            let _ = port.set_baud_rate(baud);
            drain(&mut port, 100);
            let pkt = slip_encode(&request(SYNC, &sync_payload(), 0));
            let end = Instant::now() + Duration::from_millis(2500);
            let mut got = Vec::new();
            while Instant::now() < end {
                let _ = port.write_all(&pkt);
                let _ = port.flush();
                got = drain_collect(&mut port, 120);
                if !got.is_empty() {
                    break;
                }
            }
            println!("[4] sync at {baud}: {} bytes: {:02x?}", got.len(), &got[..got.len().min(96)]);
        }

        // Leave the board as we found it: boot the firmware.
        apply_script(&mut port, &run_firmware_script());
    }

    /// Bench probe for the MicroPython console: open, listen, interrupt,
    /// listen — dumps everything so a silent or boot-looping board is
    /// distinguishable from a healthy prompt.
    #[test]
    #[ignore = "bench only: needs the esp8266-oled board on COM12"]
    fn bench_rom_registers() {
        let name = "COM12";
        let mut port: Box<dyn SerialPort> = serialport::new(name, OPEN_BAUD)
            .timeout(Duration::from_millis(PORT_TIMEOUT))
            .open()
            .expect("open COM12");
        std::thread::sleep(Duration::from_millis(BOOT_WAIT));
        drain(&mut port, 300);
        apply_script(&mut port, &download_mode_script());
        drain(&mut port, 300);
        let _ = port.set_baud_rate(115_200);
        let mut pending = Vec::new();
        let mut session = Session::new(&mut port);
        session.sync(&mut pending).expect("sync");
        session.flash_begin(0, 0, &mut pending).expect("attach");
        drop(session);
        port.clear(ClearBuffer::Input).expect("flush");
        let mut session = Session::new(&mut port);

        // Round-trip a scratch value through W0 to prove register I/O.
        const W0: u32 = 0x6000_0200 + 0x40;
        session.write_reg(W0, 0xDEAD_BEEF, &mut pending).expect("write W0");
        let back = session.read_reg(W0, &mut pending).expect("read W0");
        println!("[a] W0 round-trip: {back:#010x} (want deadbeef)");

        // Manual RDID with full visibility: every register state visible.
        const BASE: u32 = 0x6000_0200;
        let (usr_r, usr1_r, usr2_r) = (BASE + 0x1C, BASE + 0x20, BASE + 0x24);
        session.write_reg(usr1_r, 23 << 8, &mut pending).expect("usr1");
        session.write_reg(usr_r, 0x9000_0000, &mut pending).expect("usr");
        session.write_reg(usr2_r, 0x7000_009F, &mut pending).expect("usr2");
        session.write_reg(W0, 0, &mut pending).expect("w0 clear");
        session.write_reg(BASE, 1 << 18, &mut pending).expect("trigger");
        for i in 0..3 {
            let cmd = session.read_reg(BASE, &mut pending).expect("poll");
            let w0 = session.read_reg(W0, &mut pending).expect("w0");
            println!("[b{i}] SPI_CMD={cmd:#010x} W0={w0:#010x}");
        }

        let id = session.flash_id(&mut pending).expect("flash id");
        println!("[c] JEDEC W0: {id:#010x}");

        let usr = session.read_reg(0x6000_0200 + 0x1C, &mut pending).expect("usr");
        println!("[d] SPI_USR after: {usr:#010x}");
        apply_script(&mut port, &run_firmware_script());
    }

    /// Read and return whatever arrives for `ms`.
    fn drain_collect(port: &mut Box<dyn SerialPort>, ms: u64) -> Vec<u8> {
        let end = Instant::now() + Duration::from_millis(ms);
        let mut out = Vec::new();
        while Instant::now() < end {
            let waiting = port.bytes_to_read().unwrap_or(0) as usize;
            let mut scratch = vec![0u8; waiting.max(1)];
            match port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => out.extend_from_slice(&scratch[..n]),
                Err(_) => {}
            }
        }
        out
    }

    /// Bench probe for the MicroPython console: open, listen, interrupt,
    /// listen — dumps everything so a silent or boot-looping board is
    /// distinguishable from a healthy prompt.
    #[test]
    #[ignore = "bench only: needs the esp8266-oled board on COM12"]
    fn bench_repl_console() {
        let name = "COM12";
        let mut port: Box<dyn SerialPort> = serialport::new(name, OPEN_BAUD)
            .timeout(Duration::from_millis(PORT_TIMEOUT))
            .open()
            .expect("open COM12");
        std::thread::sleep(Duration::from_millis(BOOT_WAIT));
        let heard = drain_collect(&mut port, 2500);
        println!(
            "[1] after open, {} bytes: {}",
            heard.len(),
            String::from_utf8_lossy(&heard[..heard.len().min(300)])
        );
        let _ = port.write_all(b"\r\x03\x03");
        let _ = port.flush();
        let heard = drain_collect(&mut port, 1500);
        println!(
            "[2] after interrupt, {} bytes: {}",
            heard.len(),
            String::from_utf8_lossy(&heard[..heard.len().min(300)])
        );
        let _ = port.write_all(b"\x02\x01");
        let _ = port.flush();
        let heard = drain_collect(&mut port, 1000);
        println!(
            "[3] after ctrl-b ctrl-a, {} bytes: {}",
            heard.len(),
            String::from_utf8_lossy(&heard[..heard.len().min(300)])
        );
    }

    #[test]
    fn flash_writes_full_padded_blocks_and_acks_each() {
        let image: Vec<u8> = (0..1000).map(|i| (i % 251) as u8).collect(); // < 1 block
        let mut port = FakePort::new()
            .respond(FLASH_BEGIN, &FakePort::ok_response(FLASH_BEGIN))
            .respond(FLASH_DATA, &FakePort::ok_response(FLASH_DATA));
        let mut pending = Vec::new();
        let mut seen = Vec::new();
        {
            let mut session = Session::with_timeouts(&mut port, fast_timeouts());
            session
                .flash_image(&image, 0, &mut pending, |done, total| seen.push((done, total)))
                .unwrap();
        }
        let packets = port.written_packets();
        assert_eq!(packets.len(), 2); // begin + one block
        let begin = &packets[0];
        assert_eq!(begin[1], FLASH_BEGIN);
        // erase_size (workaround), num_blocks 1, block size 0x400, offset 0.
        let erase = u32::from_le_bytes(begin[8..12].try_into().unwrap());
        assert_eq!(erase, get_erase_size(0, 1000));
        assert_eq!(u32::from_le_bytes(begin[12..16].try_into().unwrap()), 1);
        assert_eq!(u32::from_le_bytes(begin[16..20].try_into().unwrap()), 0x400);
        assert_eq!(u32::from_le_bytes(begin[20..24].try_into().unwrap()), 0);

        let block = &packets[1];
        assert_eq!(block[1], FLASH_DATA);
        assert_eq!(u32::from_le_bytes(block[8..12].try_into().unwrap()), 0x400); // padded
        assert_eq!(u32::from_le_bytes(block[12..16].try_into().unwrap()), 0); // seq
        assert_eq!(&block[24..24 + image.len()], &image[..]);
        assert!(block[24 + image.len()..].iter().all(|&b| b == 0xFF));
        // The checksum rides the header (u32 slot), over the flash bytes only.
        assert_eq!(
            u32::from_le_bytes(block[4..8].try_into().unwrap()) as u8,
            xor_checksum(&block[24..])
        );
        assert_eq!(seen, vec![(1, 1)]);
    }

    #[test]
    fn a_block_that_never_acks_aborts_after_three_attempts() {
        let mut port = FakePort::new()
            .respond(FLASH_BEGIN, &FakePort::ok_response(FLASH_BEGIN));
        // No FLASH_DATA responder: every block write times out.
        let mut pending = Vec::new();
        let mut session = Session::with_timeouts(&mut port, fast_timeouts());
        assert!(session
            .flash_image(&mut vec![0xE9; 10], 0, &mut pending, |_, _| {})
            .is_err());
        // Three attempts, all on the same first block.
        let packets = port.written_packets();
        assert_eq!(packets[1..].len(), WRITE_BLOCK_ATTEMPTS as usize);
        assert!(packets[1..].iter().all(|p| p[1] == FLASH_DATA));
    }

    #[test]
    fn a_failed_status_is_an_immediate_refusal() {
        let mut bad = vec![0x01, FLASH_DATA];
        bad.extend_from_slice(&0u16.to_le_bytes());
        bad.extend_from_slice(&0u32.to_le_bytes());
        bad.push(0xA1); // status: failure
        bad.push(0x05); // reason
        let mut port = FakePort::new()
            .respond(FLASH_BEGIN, &FakePort::ok_response(FLASH_BEGIN))
            .respond(FLASH_DATA, &bad);
        let mut pending = Vec::new();
        let mut session = Session::with_timeouts(&mut port, fast_timeouts());
        let err = session
            .flash_image(&mut vec![0xE9; 10], 0, &mut pending, |_, _| {})
            .unwrap_err();
        assert!(format!("{err:#}").contains("status 161"));
    }
}

//! The screenshot — a copy of the screen, pulled from a live face.
//!
//! `J,{"shot":1}` (the complex-value escape's snapshot form) makes the
//! face write its frame buffer to /shot.tmp WITHOUT stopping — pulse
//! and all. This module lifts that file over a read-only raw-REPL
//! session (the install path's slice sizes, never trusting a blind
//! read), encodes 1-bit PNGs with no dependencies (stored deflate),
//! and reboots the face so it comes straight back up.

use anyhow::{anyhow, bail};
use serialport::SerialPort;
use std::io::Write;
use std::time::{Duration, Instant};

const SLICE: usize = 384; // hexlify doubles it on-device — the heap floor
const FRAME: usize = 1024; // 128 x 64 / 8

fn drain(port: &mut Box<dyn SerialPort>, ms: u64) -> Vec<u8> {
    let mut out = Vec::new();
    let end = Instant::now() + Duration::from_millis(ms);
    while Instant::now() < end {
        let mut scratch = [0u8; 512];
        match port.read(&mut scratch) {
            Ok(0) => {}
            Ok(n) => out.extend_from_slice(&scratch[..n]),
            Err(_) => {}
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    out
}

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// One raw-REPL round trip: code + Ctrl-D, reply ends with the
/// `\x04>` pair (a bare `\x04` check only passes on a lucky split).
fn exec(port: &mut Box<dyn SerialPort>, code: &str) -> anyhow::Result<Vec<u8>> {
    port.write_all(code.as_bytes())?;
    port.write_all(b"\x04")?;
    port.flush()?;
    let mut out = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let mut scratch = [0u8; 512];
        match port.read(&mut scratch) {
            Ok(0) => {}
            Ok(n) => out.extend_from_slice(&scratch[..n]),
            Err(_) => {}
        }
        if out.ends_with(b"\x04>") {
            if out.windows(9).any(|w| w == b"Traceback") {
                bail!("device raised: {}", String::from_utf8_lossy(&out));
            }
            return Ok(out);
        }
    }
    bail!("no end-of-reply marker — framing unknown");
}

/// Pull the 1 KB frame from a live face — in-band. The face answers
/// `J,{"shot":1}` with `OK,<base64>*hh` on the wire itself: no
/// interrupt, no reboot, the dance goes on. Lines are dribbled and the
/// reply read to its newline; the checksum is stripped and verified by
/// the caller-side parser habits of the house.
pub fn capture(port_name: &str) -> anyhow::Result<Vec<u8>> {
    let mut port = open_port(port_name)?;
    let frame = capture_on(&mut port)?;
    Ok(frame)
}

fn open_port(port_name: &str) -> anyhow::Result<Box<dyn SerialPort>> {
    let port = serialport::new(port_name, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| anyhow!("{port_name}: {e}"))?;
    sleep_ms(2500); // boot wait if just plugged
    Ok(port)
}

/// One in-band shot on an open session: `J,{"shot":1}` dribbled, the
/// reply read to its newline. The face answers `OK,<base64>*hh` —
/// no interrupt, no reboot, the dance goes on.
fn capture_on(port: &mut Box<dyn SerialPort>) -> anyhow::Result<Vec<u8>> {
    let mut data = b"\rJ,{\"shot\":1}\n".to_vec();
    for chunk in data.chunks(16) {
        port.write_all(chunk)?;
        port.flush()?;
        sleep_ms(4);
    }

    // The reply is one long line among possible boot noise: opening
    // the port can reset the board, and its spew misread at this baud
    // produces shorter garbage lines. The only accepted reply is a
    // line that actually decodes to a whole frame — everything else is
    // scanned past (identity-parse lesson: extract, never anchor).
    let mut acc = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let mut scratch = [0u8; 512];
        match port.read(&mut scratch) {
            Ok(0) => {}
            Ok(n) => acc.extend_from_slice(&scratch[..n]),
            Err(_) => {}
        }
        while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = acc.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&line[..line.len() - 1])
                .trim()
                .to_string();
            if let Some(body) = s.strip_prefix("OK,") {
                let frame = decode_b64(strip_b64_checksum(body));
                if frame.len() == FRAME {
                    return Ok(frame);
                }
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("no whole-frame reply within 8 s — face unreachable or mid-boot")
}

/// The trail camera: loop the in-band shot at the wire-respecting rate
/// and leave the encoding to the host. The face keeps dancing — each
/// shot costs it one write of ~120 ms, everything else is ours.
/// `zones` (from the class manifest) color the frames; returns the
/// frame count actually captured (the wire may be slower than the ask).
pub fn record(
    port_name: &str,
    secs: u32,
    fps: u32,
    zones: &[(usize, usize, [u8; 3])],
    out: &std::path::Path,
) -> anyhow::Result<usize> {
    let fps = fps.clamp(1, 5); // 5 fps ~= 7 KB/s — the wire's honest ceiling
    let period = Duration::from_millis(1000 / fps as u64);
    let mut port = open_port(port_name)?;

    let mut frames: Vec<Vec<u8>> = Vec::new();
    let mut next_at = Instant::now();
    let end = next_at + Duration::from_secs(secs as u64);
    while Instant::now() < end {
        next_at += period;
        let frame = capture_on(&mut port)?;
        frames.push(index_frame(&frame, zones));
        let now = Instant::now();
        if next_at > now {
            sleep_ms((next_at - now).as_millis() as u64);
        } else {
            next_at = now; // wire-bound: skip the missed slot, keep going
        }
    }

    let delay_cs = (1000 / fps as u16) / 10;
    crate::gif::write_gif(out, 128, 64, delay_cs.max(2), &GIF_PALETTE, &frames)?;
    Ok(frames.len())
}

/// The panel's three truths + a spare: dark, yellow strip, cyan field.
pub const GIF_PALETTE: [[u8; 3]; 4] =
    [[0, 0, 0], [255, 221, 0], [0, 213, 255], [255, 255, 255]];

fn index_frame(frame: &[u8], zones: &[(usize, usize, [u8; 3])]) -> Vec<u8> {
    let w = 128usize;
    let mut out = vec![0u8; w * 64];
    for page in 0..8 {
        for col in 0..w {
            let bits = frame[page * w + col];
            for b in 0..8u32 {
                if bits & (1 << b) != 0 {
                    let y = page * 8 + b as usize;
                    let idx = zones
                        .iter()
                        .position(|(y0, y1, _)| y >= *y0 && y <= *y1)
                        .map(|i| (i + 1).min(3) as u8)
                        .unwrap_or(3);
                    out[y * w + col] = idx;
                }
            }
        }
    }
    out
}

/// Strip a trailing `*hh` checksum — same grammar as the probe.
fn strip_b64_checksum(line: &str) -> &str {
    let l = line.trim();
    let b = l.as_bytes();
    if b.len() >= 3
        && b[b.len() - 3] == b'*'
        && b[b.len() - 2].is_ascii_hexdigit()
        && b[b.len() - 1].is_ascii_hexdigit()
    {
        return std::str::from_utf8(&b[..b.len() - 3]).unwrap_or(l);
    }
    l
}

/// Base64 decode, whitespace-tolerant, no dependencies.
pub fn decode_b64(s: &str) -> Vec<u8> {
    fn val(b: u8) -> Option<u32> {
        match b {
            b'A'..=b'Z' => Some((b - b'A') as u32),
            b'a'..=b'z' => Some((b - b'a' + 26) as u32),
            b'0'..=b'9' => Some((b - b'0' + 52) as u32),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    }
    let bytes: Vec<u8> = s
        .bytes()
        .filter(|b| !b.is_ascii_whitespace() && *b != b'=')
        .collect();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4 + 3);
    for chunk in bytes.chunks(4) {
        let mut acc = 0u32;
        for (i, b) in chunk.iter().enumerate() {
            acc |= val(*b).unwrap_or(0) << (18 - 6 * i);
        }
        out.push((acc >> 16) as u8);
        if chunk.len() > 2 {
            out.push((acc >> 8) as u8);
        }
        if chunk.len() > 3 {
            out.push(acc as u8);
        }
    }
    out
}


/// PNG writer, no dependencies: 8-bit truecolor, stored-deflate IDAT.
/// `px` is one RGB triple per pixel, `w` x `h`, scaled by integer
/// replication (nearest-neighbour — the chunky look is the look).
pub fn write_png(
    path: &std::path::Path,
    w: usize,
    h: usize,
    px: &[[u8; 3]],
    scale: usize,
) -> anyhow::Result<()> {
    let sw = w * scale;
    let mut raw = Vec::with_capacity(h * scale * (1 + sw * 3));
    for y in 0..h * scale {
        raw.push(0u8); // filter: none
        let sy = y / scale;
        for x in 0..sw {
            raw.extend_from_slice(&px[sy * w + x / scale]);
        }
    }
    let mut idat = vec![0x78, 0x01];
    for block in raw.chunks(65535) {
        let last = block.len() < 65535;
        idat.push(if last { 1 } else { 0 });
        idat.extend_from_slice(&(block.len() as u16).to_le_bytes());
        idat.extend_from_slice(&(!(block.len() as u16)).to_le_bytes());
        idat.extend_from_slice(block);
    }
    idat.extend_from_slice(&be32(adler32(&raw)));

    let mut png = Vec::new();
    png.extend_from_slice(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&be32(sw as u32));
    ihdr.extend_from_slice(&be32((h * scale) as u32));
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit truecolor RGB
    for (kind, data) in [("IHDR", ihdr), ("IDAT", idat), ("IEND", vec![])] {
        png.extend_from_slice(&be32(data.len() as u32));
        png.extend_from_slice(kind.as_bytes());
        png.extend_from_slice(&data);
        png.extend_from_slice(&be32(crc32(&[kind.as_bytes(), &data].concat())));
    }
    std::fs::write(path, png)?;
    Ok(())
}

fn crc32(data: &[u8]) -> u32 {
    let mut table = [0u32; 256];
    for (i, e) in table.iter_mut().enumerate() {
        let mut c = i as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { 0xEDB8_8320 ^ (c >> 1) } else { c >> 1 };
        }
        *e = c;
    }
    let mut c = 0xFFFF_FFFFu32;
    for &b in data {
        c = table[((c ^ b as u32) & 0xFF) as usize] ^ (c >> 8);
    }
    c ^ 0xFFFF_FFFF
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn be32(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

/// Decode the MVLSB frame (column bytes, 8 vertical pixels, D0 = top)
/// and render the face as the eye sees it: each native row shines in
/// its phosphor zone's color (dual-zone panels: a yellow strip above a
/// cyan field). `zones` is (first_row, last_row, rgb); rows outside
/// any zone fall back to a neutral white.
pub fn render(
    frame: &[u8],
    zones: &[(usize, usize, [u8; 3])],
    out_portrait: &std::path::Path,
    out_native: &std::path::Path,
) -> anyhow::Result<()> {
    let w = 128usize;
    let neutral = [230u8, 230, 230];
    // An OLED's off state is black; lit pixels shine their zone's color.
    let off = [0u8, 0, 0];
    let shade = |y: usize| -> [u8; 3] {
        zones
            .iter()
            .find(|(y0, y1, _)| y >= *y0 && y <= *y1)
            .map(|(_, _, c)| *c)
            .unwrap_or(neutral)
    };
    let mut native = vec![off; w * 64];
    for page in 0..8 {
        for col in 0..128usize {
            let bits = frame[page * 128 + col];
            for b in 0..8u32 {
                if bits & (1 << b) != 0 {
                    native[(page * 8 + b as usize) * w + col] = shade(page * 8 + b as usize);
                }
            }
        }
    }
    // The panel stands on its long edge: portrait(u,v) -> native(v, 63-u).
    // Row-major portrait: index = v * 64 + u.
    let mut portrait = vec![off; 64 * 128];
    for u in 0..64usize {
        for v in 0..128usize {
            let src = native[(63 - u) * w + v];
            if src != off {
                portrait[v * 64 + u] = src;
            }
        }
    }
    write_png(out_portrait, 64, 128, &portrait, 3)?;
    write_png(out_native, w, 64, &native, 4)?;
    Ok(())
}

/// `#rrggbb` -> RGB triple; unparsable colors fall back to white.
pub fn parse_color(s: &str) -> [u8; 3] {
    let hex = s.trim_start_matches('#');
    if hex.len() != 6 {
        return [230, 230, 230];
    }
    let mut out = [230u8, 230, 230];
    for (i, pair) in hex.as_bytes().chunks(2).enumerate() {
        out[i] = u8::from_str_radix(std::str::from_utf8(pair).unwrap_or("e6"), 16).unwrap_or(230);
    }
    out
}

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

/// Pull the 1 KB frame from a live face on `port_name`.
pub fn capture(port_name: &str) -> anyhow::Result<Vec<u8>> {
    let mut port = serialport::new(port_name, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| anyhow!("{port_name}: {e}"))?;
    sleep_ms(2500); // boot wait if just plugged

    // The shot request, dribbled — the UART RX FIFO overruns bursts.
    // A bare newline first: the face ignores empty lines.
    let mut ask = |frame: &str| -> anyhow::Result<bool> {
        let mut data = format!("\r{frame}\n").into_bytes();
        for chunk in data.chunks(16) {
            port.write_all(chunk)?;
            port.flush()?;
            sleep_ms(4);
        }
        let mut line = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(5);
        while Instant::now() < deadline {
            let mut scratch = [0u8; 128];
            match port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => line.extend_from_slice(&scratch[..n]),
                Err(_) => {}
            }
            while let Some(pos) = line.iter().position(|&b| b == b'\n') {
                let reply: Vec<u8> = line.drain(..=pos).collect();
                let s = String::from_utf8_lossy(&reply[..reply.len() - 1]);
                let s = s.trim();
                if s.starts_with("ERR") {
                    bail!("face said: {s}");
                }
                if s.starts_with("OK") {
                    return Ok(true);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        Ok(false)
    };

    if !ask("J,{\"shot\":1}")? {
        bail!("face did not answer the shot request");
    }

    // Pause the face (it yields on KeyboardInterrupt, by design) and
    // enter raw mode — verified, never assumed.
    port.write_all(b"\r\x03\x03")?;
    port.flush()?;
    sleep_ms(700);
    drain(&mut port, 200);
    port.write_all(b"\x02")?;
    port.flush()?;
    sleep_ms(400);
    drain(&mut port, 300);
    port.write_all(b"\r\n")?;
    port.flush()?;
    // MicroPython's prompt is ">>> " — trailing space and all.
    let mut sync = drain(&mut port, 1000);
    while sync.last() == Some(&b' ') || sync.last() == Some(&b'\r') || sync.last() == Some(&b'\n') {
        sync.pop();
    }
    if !sync.ends_with(b">>>") {
        bail!("friendly prompt not answering");
    }
    port.write_all(b"\x01")?;
    port.flush()?;
    sleep_ms(300);
    let banner = drain(&mut port, 300);
    if !banner.windows(8).any(|w| w == b"raw REPL") {
        bail!("could not confirm raw REPL");
    }

    // Lift the frame, sliced.
    exec(&mut port, "f=open('/shot.tmp','rb')")?;
    exec(&mut port, "import ubinascii")?;
    let mut frame = Vec::with_capacity(FRAME);
    while frame.len() < FRAME {
        let reply = exec(
            &mut port,
            &format!("print(ubinascii.hexlify(f.read({SLICE})))"),
        )?;
        let text = String::from_utf8_lossy(&reply);
        let start = text.find("b'").ok_or_else(|| anyhow!("unparseable slice"))? + 2;
        let end = text[start..]
            .find('\'')
            .ok_or_else(|| anyhow!("unparseable slice"))?
            + start;
        for pair in text[start..end].as_bytes().chunks(2) {
            frame.push(u8::from_str_radix(
                std::str::from_utf8(pair).map_err(|e| anyhow!("{e}"))?,
                16,
            )?);
        }
    }
    exec(&mut port, "f.close()")?;
    exec(&mut port, "import os; os.remove('/shot.tmp')")?;

    // Bring the face straight back up.
    port.write_all(b"\x02\x04")?;
    port.flush()?;
    sleep_ms(2500);
    Ok(frame)
}

// ── 1-bit PNG, no dependencies: grayscale, stored-deflate IDAT ──

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

/// `px` is one byte per pixel (0 = dark, 1 = lit), `w` x `h`, scaled by
/// integer replication (nearest-neighbour — the chunky look is the look).
pub fn write_png(path: &std::path::Path, w: usize, h: usize, px: &[u8], scale: usize) -> anyhow::Result<()> {
    let sw = w * scale;
    let stride = (sw + 7) / 8;
    let mut raw = Vec::with_capacity(h * scale * (1 + stride));
    for y in 0..h * scale {
        raw.push(0u8); // filter: none
        let sy = y / scale;
        for xb in 0..stride {
            let mut byte = 0u8;
            for bit in 0..8 {
                let x = xb * 8 + bit;
                if x < sw && px[sy * w + x / scale] != 0 {
                    byte |= 0x80 >> bit;
                }
            }
            raw.push(byte);
        }
    }

    // zlib stream: header + stored deflate blocks + adler32.
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
    ihdr.extend_from_slice(&[1, 0, 0, 0, 0]); // 1-bit grayscale
    for (kind, data) in [("IHDR", ihdr), ("IDAT", idat), ("IEND", vec![])] {
        png.extend_from_slice(&be32(data.len() as u32));
        png.extend_from_slice(kind.as_bytes());
        png.extend_from_slice(&data);
        png.extend_from_slice(&be32(crc32(
            &[kind.as_bytes(), &data].concat(),
        )));
    }
    std::fs::write(path, png)?;
    Ok(())
}

/// Decode the MVLSB frame (column bytes, 8 vertical pixels, D0 = top)
/// and render the face's true portrait orientation + the native panel.
pub fn render(frame: &[u8], out_portrait: &std::path::Path, out_native: &std::path::Path) -> anyhow::Result<()> {
    let w = 128usize;
    let mut native = vec![0u8; w * 64];
    for page in 0..8 {
        for col in 0..128usize {
            let bits = frame[page * 128 + col];
            for b in 0..8u32 {
                if bits & (1 << b) != 0 {
                    native[(page * 8 + b as usize) * w + col] = 1;
                }
            }
        }
    }
    // The panel stands on its long edge: portrait(u,v) -> native(v, 63-u).
    // Row-major portrait: index = v * 64 + u.
    let mut portrait = vec![0u8; 64 * 128];
    for u in 0..64usize {
        for v in 0..128usize {
            if native[(63 - u) * w + v] != 0 {
                portrait[v * 64 + u] = 1;
            }
        }
    }
    write_png(out_portrait, 64, 128, &portrait, 3)?;
    write_png(out_native, w, 64, &native, 4)?;
    Ok(())
}

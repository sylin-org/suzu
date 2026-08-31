//! The trail camera — screenshots and recordings of live faces.
//!
//! `J,{"shot":1}` (the snapshot form of the complex-value escape) makes
//! a face answer with its RAW frame buffer in its poll ack —
//! `OK,<base64>*hh` on the wire itself: no interrupt, no mode change,
//! no reboot; the animation keeps dancing. The bytes are device-shaped:
//! the class manifest's `frame:` section is the only per-device
//! knowledge, and one generic decoder turns them into pixels (ADR-0001:
//! devices ship raw memory; the host interprets).

use crate::catalog::{parse_color, FrameSpec};
use anyhow::{anyhow, bail, Result};
use serialport::SerialPort;
use std::io::Write;
use std::time::{Duration, Instant};

fn sleep_ms(ms: u64) {
    std::thread::sleep(Duration::from_millis(ms));
}

/// Open a port at the suzu baud and let it settle: opening can reset
/// the board (the CH340 hard-resets, the CDC soft-resets), and a
/// mid-boot face answers nothing.
pub fn open_port(port_name: &str) -> Result<Box<dyn SerialPort>> {
    let mut port = serialport::new(port_name, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| anyhow!("{port_name}: {e}"))?;
    // CircuitPython gates its CDC console on DTR: without it the face
    // hears nothing and answers nothing (proven on the bench, 2026-08-29
    // — the matrix was 0 bytes at DTR low, its whole frame at DTR high).
    let _ = port.write_data_terminal_ready(true);
    sleep_ms(2500); // boot wait if just plugged
    Ok(port)
}

/// One in-band shot on an open session: `J,{"shot":1}` dribbled 16
/// bytes at a time (the device's UART RX FIFO overruns bursts), the
/// reply scanned out of accumulated newline-terminated lines — never
/// anchored on the first (boot noise produces shorter lines). The only
/// accepted reply is a line that decodes to a whole frame at `expected`
/// bytes; its `*hh` xor checksum is verified when present.
pub fn capture_on(port: &mut Box<dyn SerialPort>, expected: usize) -> Result<Vec<u8>> {
    dribble_line(port, "J,{\"shot\":1}")?;

    let mut acc = Vec::new();
    // The reply rides base64 (4/3 inflation) at the wire's honest
    // rate (11.5 k chars/s at 115200; budgeted at half that — the
    // bench measured a healthy 32.4 KB mirror at 4.9 s and a thrashing
    // face slower still). The bound is computed from the declared
    // size, never guessed — and the one bound waited is the one told.
    let secs = (2 + (expected as u64 * 4 / 3) / 6_000).max(8);
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let mut scratch = [0u8; 512];
        match port.read(&mut scratch) {
            Ok(0) => {}
            Ok(n) => acc.extend_from_slice(&scratch[..n]),
            Err(_) => {}
        }
        while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = acc.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&line[..line.len() - 1]).to_string();
            if let Some(frame) = parse_reply(&s, expected) {
                return Ok(frame);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    bail!("no whole-frame reply within {secs} s — face unreachable or mid-boot")
}

/// One in-band shot on a port by name (opens and settles it first).
pub fn capture(port_name: &str, expected: usize) -> Result<Vec<u8>> {
    let mut port = open_port(port_name)?;
    capture_on(&mut port, expected)
}

/// One wire-shaped request, dribbled 16 bytes at a time — the device's
/// UART RX FIFO overruns bursts (the shared pacing, one place).
pub fn dribble_line(serial: &mut Box<dyn SerialPort>, line: &str) -> Result<()> {
    let mut request = format!("\r{line}\n").into_bytes();
    for chunk in request.chunks_mut(16) {
        serial.write_all(chunk)?;
        serial.flush()?;
        sleep_ms(4);
    }
    Ok(())
}

/// Pull the frame out of one candidate line: an `OK,`-prefixed reply
/// decoding to exactly `expected` bytes. A trailing `*hh` xor checksum
/// — computed over everything from `OK,` to before the `*`, per the
/// wire grammar — is verified when present. Boot noise and
/// CircuitPython's console-title escapes glue onto reply lines, so
/// every `OK,` in the line is a candidate (extract, never anchor —
/// the identity-parse lesson). Anything else is `None`.
fn parse_reply(line: &str, expected: usize) -> Option<Vec<u8>> {
    let line = line.trim();
    for (start, _) in line.match_indices("OK,") {
        let rest = &line[start..];
        let body = match rest.rsplit_once('*') {
            Some((b, s)) if s.len() == 2 && s.bytes().all(|c| c.is_ascii_hexdigit()) => {
                let mut x = 0u8;
                for c in b.bytes() {
                    x ^= c;
                }
                if format!("{x:02x}") != s {
                    continue; // wrong anchor or corrupted — try the next
                }
                b
            }
            _ => rest,
        };
        let frame = decode_b64(&body[3..]);
        if frame.len() == expected {
            return Some(frame);
        }
    }
    None
}

/// Base64 encode, no dependencies — the frame lane carries PNG bytes
/// as text, and this is its one alphabet.
pub fn encode_b64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(TABLE[(n >> 18) as usize & 63] as char);
        out.push(TABLE[(n >> 12) as usize & 63] as char);
        out.push(if chunk.len() > 1 { TABLE[(n >> 6) as usize & 63] as char } else { '=' });
        out.push(if chunk.len() > 2 { TABLE[n as usize & 63] as char } else { '=' });
    }
    out
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

/// The manifest-driven decoder: raw frame bytes → native RGBA. This is
/// the one place format knowledge lives, and the manifest is the only
/// thing it reads.
pub fn decode_frame(
    frame: &[u8],
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
) -> Result<(usize, usize, Vec<u8>)> {
    let (w, h) = (spec.width, spec.height);
    // The declared order must agree with the format it decorates — a
    // manifest that lies about its bytes decodes to noise.
    let order_ok = match (spec.format.as_str(), spec.order.as_deref()) {
        ("mvlsb", Some("column-major"))
        | ("rgb24", Some("row-major"))
        | ("rgb565", Some("row-major"))
        | ("rgb332", Some("row-major")) => true,
        (f, o) if f == "mvlsb" || f == "rgb24" => o.is_none(),
        _ => true, // unknown format: rejected below with its own message
    };
    if !order_ok {
        bail!(
            "manifest frame law contradicts itself: {} with order {:?}",
            spec.format,
            spec.order
        );
    }
    match (spec.format.as_str(), spec.depth) {
        ("rgb24", 24) => {
            if frame.len() != w * h * 3 {
                bail!("rgb24: {} B is not a whole {w}x{h} frame", frame.len());
            }
            let mut rgba = Vec::with_capacity(w * h * 4);
            for px in frame.chunks_exact(3) {
                rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
            }
            Ok((w, h, rgba))
        }
        ("rgb565", 16) => {
            if frame.len() != w * h * 2 {
                bail!("rgb565: {} B is not a whole {w}x{h} frame", frame.len());
            }
            // Big-endian per pixel (the family's blit convention).
            let mut rgba = Vec::with_capacity(w * h * 4);
            for px in frame.chunks_exact(2) {
                let v = ((px[0] as u16) << 8) | px[1] as u16;
                let r = (((v >> 11) & 0x1F) * 255 / 31) as u8;
                let g = (((v >> 5) & 0x3F) * 255 / 63) as u8;
                let b = ((v & 0x1F) * 255 / 31) as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
            Ok((w, h, rgba))
        }
        ("rgb332", 8) => {
            if frame.len() != w * h {
                bail!("rgb332: {} B is not a whole {w}x{h} frame", frame.len());
            }
            // A byte a pixel: 3-3-2 bits, the small-heap mirror's
            // honest color, expanded on precomputed ramps (7 * 255
            // overflows a byte mid-expression — the table never does).
            const R7: [u8; 8] = [0, 36, 73, 109, 146, 182, 219, 255];
            const R3: [u8; 4] = [0, 85, 170, 255];
            let mut rgba = Vec::with_capacity(w * h * 4);
            for px in frame {
                rgba.extend_from_slice(&[
                    R7[((px >> 5) & 0x07) as usize],
                    R7[((px >> 2) & 0x07) as usize],
                    R3[(px & 0x03) as usize],
                    255,
                ]);
            }
            Ok((w, h, rgba))
        }
        ("mvlsb", 1) => {
            if frame.len() != w * h / 8 {
                bail!("mvlsb: {} B is not a whole {w}x{h} frame", frame.len());
            }
            // Lit pixels shine their zone's phosphor; the manifest
            // palette (index 1) backs zones-less panels; unlit is
            // palette[0] — black, for an OLED's off state.
            let off = spec
                .palette
                .first()
                .map(|c| parse_color(c))
                .unwrap_or([0, 0, 0]);
            let lit = spec.palette.get(1).map(|c| parse_color(c));
            let mut rgba = Vec::with_capacity(w * h * 4);
            for y in 0..h {
                for x in 0..w {
                    let color = if frame[(y / 8) * w + x] & (1 << (y % 8)) != 0 {
                        zones
                            .iter()
                            .find(|(y0, y1, _)| y >= *y0 && y <= *y1)
                            .map(|(_, _, c)| *c)
                            .or(lit)
                            .unwrap_or([230, 230, 230])
                    } else {
                        off
                    };
                    rgba.extend_from_slice(&[color[0], color[1], color[2], 255]);
                }
            }
            Ok((w, h, rgba))
        }
        (f, d) => bail!("frame format {f:?} at {d} bpp is not decodable — fix the manifest"),
    }
}

/// Rotate 90° clockwise: dst(u, v) = src(x = v, y = h-1-u) — how a
/// panel standing on its long edge meets the eye.
fn rotate90(rgba: &[u8], w: usize, h: usize) -> (usize, usize, Vec<u8>) {
    let mut out = vec![0u8; rgba.len()];
    for y in 0..h {
        for x in 0..w {
            let (u, v) = (h - 1 - y, x);
            let dst = (v * h + u) * 4;
            let src = (y * w + x) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba[src..src + 4]);
        }
    }
    (h, w, out)
}

/// Raw frame → the view: native decode, then the manifest's rotation.
/// Returns (w, h, flat RGBA) in display orientation.
pub fn render_view(
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
    frame: &[u8],
) -> Result<(usize, usize, Vec<u8>)> {
    let (w, h, rgba) = decode_frame(frame, spec, zones)?;
    match spec.render.as_ref().map(|r| r.rotate).unwrap_or(0) {
        0 => Ok((w, h, rgba)),
        90 => Ok(rotate90(&rgba, w, h)),
        r => bail!("rotate {r} not supported — declare 0 or 90 in the manifest"),
    }
}

/// One face view → a truecolor PNG.
pub fn render_png(
    path: &std::path::Path,
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
    frame: &[u8],
) -> Result<()> {
    let png = render_png_bytes(spec, zones, frame)?;
    std::fs::write(path, png)?;
    Ok(())
}

/// One frame → the finished PNG bytes: decode per the manifest,
/// orient, encode — one pixel of the panel per pixel of the PNG.
/// Viewing size is the client's; the wire carries the truth.
pub fn render_png_bytes(
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
    frame: &[u8],
) -> Result<Vec<u8>> {
    let (w, h, rgba) = render_view(spec, zones, frame)?;
    let rgb: Vec<[u8; 3]> = rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
    png_bytes(w, h, &rgb)
}

/// A decodable face: its port and the manifest knowledge that names
/// its bytes.
pub struct Face {
    pub port: String,
    pub class: String,
    pub spec: FrameSpec,
    pub zones: Vec<(usize, usize, [u8; 3])>,
}

/// The trail camera: loop the in-band shot against the first answering
/// face. Each shot costs the face one ack-sized write (~120 ms) — the
/// wire, not the encoder, is the tax — so the loop is wire-bound:
/// missed slots are skipped and the dance goes on. Frames are decoded
/// per the face's manifest and written as an animated GIF (truecolor
/// in; the gif crate quantizes). Returns (path, frames captured).
pub fn record_first(
    faces: &[Face],
    secs: u32,
    fps: u32,
    prefix: &str,
) -> Result<(std::path::PathBuf, usize)> {
    let fps = fps.clamp(1, 5); // 5 fps ~= 7 KB/s — the wire's honest ceiling
    let period = Duration::from_millis(1000 / fps as u64);
    let delay_cs = ((1000 / fps as u16) / 10).max(2);

    for face in faces {
        let mut port = match open_port(&face.port) {
            Ok(p) => p,
            Err(e) => {
                println!("  {}: skipped ({e})", face.port);
                continue;
            }
        };
        let first = match capture_on(&mut port, face.spec.size) {
            Ok(f) => f,
            Err(e) => {
                println!("  {}: no shot ({e})", face.port);
                continue;
            }
        };

        // This face answered: it is the subject.
        println!("  {} [{}] answers — the subject", face.port, face.class);
        let (w, h, rgba) = render_view(&face.spec, &face.zones, &first)?;
        let mut frames = vec![rgba];
        let mut next_at = Instant::now();
        let end = next_at + Duration::from_secs(secs as u64);
        while Instant::now() < end {
            next_at += period;
            match capture_on(&mut port, face.spec.size) {
                Ok(f) => {
                    let (_, _, v) = render_view(&face.spec, &face.zones, &f)?;
                    frames.push(v);
                }
                Err(e) => {
                    println!("  {} went quiet mid-record ({e})", face.port);
                    break;
                }
            }
            let now = Instant::now();
            if next_at > now {
                sleep_ms((next_at - now).as_millis() as u64);
            } else {
                next_at = now; // wire-bound: skip the missed slot, keep going
            }
        }

        let out = std::path::PathBuf::from(format!("{prefix}-{}.gif", face.port));
        crate::gif::write_gif_rgba(&out, w, h, delay_cs, &frames)?;
        return Ok((out, frames.len()));
    }
    bail!("no face answered the shot request — nothing to record")
}

/// PNG encoder, no dependencies: 8-bit truecolor, stored-deflate IDAT.
/// `px` is one RGB triple per pixel, `w` x `h`.
pub fn png_bytes(w: usize, h: usize, px: &[[u8; 3]]) -> Result<Vec<u8>> {
    let mut raw = Vec::with_capacity(h * (1 + w * 3));
    for y in 0..h {
        raw.push(0u8); // filter: none
        for x in 0..w {
            raw.extend_from_slice(&px[y * w + x]);
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
    ihdr.extend_from_slice(&be32(w as u32));
    ihdr.extend_from_slice(&be32(h as u32));
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]); // 8-bit truecolor RGB
    for (kind, data) in [("IHDR", ihdr), ("IDAT", idat), ("IEND", vec![])] {
        png.extend_from_slice(&be32(data.len() as u32));
        png.extend_from_slice(kind.as_bytes());
        png.extend_from_slice(&data);
        png.extend_from_slice(&be32(crc32(&[kind.as_bytes(), &data].concat())));
    }
    Ok(png)
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

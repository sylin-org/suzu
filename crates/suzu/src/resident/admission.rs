//! The admission test — the exam a face takes before it may join the
//! stream (ADR-0003). Non-destructive by construction: everything it
//! writes, it restores; everything it asserts, it verifies through the
//! face's own wire (the J shot decoded per the class manifest).
//!
//! Steps, adaptive to what the face declares:
//! - handshake — `I` answers a descriptor (the identity parse law)
//! - ack-law — `K` answers `OK`
//! - label-roundtrip — only when the descriptor exposes `label`:
//!   write a marker, read it back, restore the original
//! - display-truth — draw something only the tested command can draw,
//!   capture via J, and assert the pixels:
//!     · matrix: a `completion`-blue ripple — a hue no other state
//!     can produce (idle green, warm atoms and every other ring hue
//!     fall short of blue channel 80 at the half-brightness ceiling)
//!   · oled: a known ground pattern — digits light the cyan field
//!     far beyond what three idle fireflies can light

use crate::catalog::FrameSpec;
use crate::probe;
use anyhow::Result;
use serialport::SerialPort;
use serde_json::Value;
use std::io::Write;
use std::time::{Duration, Instant};

use super::events::AdmissionStep;

const HANDSHAKE_SECS: u64 = 4;

pub struct Report {
    pub passed: bool,
    pub steps: Vec<AdmissionStep>,
}

impl Report {
    fn step(&mut self, name: &str, ok: bool, detail: String) {
        self.steps.push(AdmissionStep {
            name: name.to_string(),
            ok,
            detail,
        });
    }
}

fn write_line(serial: &mut Box<dyn SerialPort>, line: &str) -> Result<()> {
    serial.write_all(line.as_bytes())?;
    serial.write_all(b"\n")?;
    serial.flush()?;
    Ok(())
}

/// Read accumulated serial until a line satisfies `pred`, or the
/// deadline passes. Never anchors on the first line — boot noise
/// produces shorter lines (the identity-parse lesson).
fn read_line_matching(
    serial: &mut Box<dyn SerialPort>,
    secs: u64,
    pred: impl Fn(&str) -> bool,
) -> Option<String> {
    let mut acc = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < deadline {
        let mut scratch = [0u8; 512];
        match serial.read(&mut scratch) {
            Ok(0) => {}
            Ok(n) => acc.extend_from_slice(&scratch[..n]),
            Err(_) => {}
        }
        while let Some(pos) = acc.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = acc.drain(..=pos).collect();
            let s = String::from_utf8_lossy(&line[..line.len() - 1]).trim().to_string();
            if pred(&s) {
                return Some(s);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    None
}

fn descriptor_of(serial: &mut Box<dyn SerialPort>) -> Option<Value> {
    write_line(serial, "I").ok()?;
    let line = read_line_matching(serial, HANDSHAKE_SECS, |l| l.contains('{'))?;
    probe::try_identity(&line)
}

/// Run the exam on an open session. `spec`/`zones` come from the
/// class manifest; a face without a decodable frame law skips the
/// display-truth step (marked so, not silently).
pub fn run(
    serial: &mut Box<dyn SerialPort>,
    class: Option<&str>,
    spec: Option<&FrameSpec>,
    zones: &[(usize, usize, [u8; 3])],
) -> Report {
    let mut report = Report { passed: true, steps: Vec::new() };
    let fail = |r: &mut Report, name: &str, detail: String| {
        r.step(name, false, detail);
        r.passed = false;
    };

    // ── handshake ──
    let Some(descriptor) = descriptor_of(serial) else {
        fail(&mut report, "handshake", "no descriptor answered `I`".into());
        return report;
    };
    report.step(
        "handshake",
        true,
        format!(
            "proto {} · device_id {}",
            descriptor.get("proto").and_then(|v| v.as_str()).unwrap_or("?"),
            descriptor.get("device_id").and_then(|v| v.as_str()).unwrap_or("?")
        ),
    );

    // ── ack law ──
    let _ = write_line(serial, "K");
    match read_line_matching(serial, HANDSHAKE_SECS, |l| l == "OK" || l.starts_with("OK,")) {
        Some(_) => report.step("ack-law", true, "K answered OK".into()),
        None => {
            fail(&mut report, "ack-law", "K answered nothing".into());
            return report;
        }
    }

    // ── label roundtrip (only when the face declares a label) ──
    let original = descriptor.get("label").and_then(|v| v.as_str()).map(|s| s.to_string());
    if let Some(original) = original {
        const MARKER: &str = "suzu-admission";
        let roundtrip = (|| -> Option<()> {
            write_line(serial, &format!("S,{MARKER}")).ok()?;
            let d = descriptor_of(serial)?;
            (d.get("label").and_then(|v| v.as_str()) == Some(MARKER)).then_some(())?;
            write_line(serial, &format!("S,{original}")).ok()?;
            let d = descriptor_of(serial)?;
            (d.get("label").and_then(|v| v.as_str()) == Some(original.as_str())).then_some(())
        })();
        match roundtrip {
            Some(()) => report.step(
                "label-roundtrip",
                true,
                format!("marker written, read back, original `{original}` restored"),
            ),
            None => {
                // The restore is attempted unconditionally — a face is
                // never left wearing the marker, pass or fail.
                let _ = write_line(serial, &format!("S,{original}"));
                fail(&mut report, "label-roundtrip", "marker did not round-trip".into());
                return report;
            }
        }
    } else {
        report.step("label-roundtrip", true, "face declares no label — skipped".to_string());
    }

    // ── display truth ──
    let truth = display_truth(serial, class, spec, zones);
    match truth {
        Ok(detail) => report.step("display-truth", true, detail),
        Err(e) => {
            fail(&mut report, "display-truth", format!("{e}"));
            return report;
        }
    }

    report
}

/// One frame through the J shot, decoded to the view's RGBA.
fn view_of(
    serial: &mut Box<dyn SerialPort>,
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
) -> Result<(usize, usize, Vec<u8>)> {
    let frame = crate::shot::capture_on(serial, spec.size)?;
    crate::shot::render_view(spec, zones, &frame)
}

fn display_truth(
    serial: &mut Box<dyn SerialPort>,
    class: Option<&str>,
    spec: Option<&FrameSpec>,
    zones: &[(usize, usize, [u8; 3])],
) -> Result<String> {
    let Some(spec) = spec else {
        return Ok("class declares no frame law — display truth not assertable".into());
    };
    match class {
        // A completion-blue ripple is a hue only a completion drop can
        // make: every other state and verb stays under blue 80 at the
        // half-brightness ceiling.
        Some("waveshare-rp2040-matrix") => {
            let _ = view_of(serial, spec, zones)?; // drain any stale frame
            crate::shot::dribble_line(serial, "R,completion.0,2,0,1,1,admission")?;
            std::thread::sleep(Duration::from_millis(300));
            let (w, _h, rgba) = view_of(serial, spec, zones)?;
            let blue = rgba.chunks_exact(4).filter(|p| p[0] < 60 && p[1] > 50 && p[2] > 80).count();
            let _ = crate::shot::dribble_line(serial, "X"); // the lake stands down
            if blue == 0 {
                anyhow::bail!("no completion-blue pixel in the {w}px view — the ripple never landed");
            }
            Ok(format!("completion ripple landed ({blue} blue px)"))
        }
        // A known ground pattern lights the cyan field far beyond what
        // three idle fireflies can light.
        Some("esp8266-oled-v2-class") => {
            crate::shot::dribble_line(serial, "G,report,88,77,66")?;
            std::thread::sleep(Duration::from_millis(400));
            let (_w, _h, rgba) = view_of(serial, spec, zones)?;
            let field = rgba.chunks_exact(4).filter(|p| p[2] > 150).count();
            if field < 150 {
                anyhow::bail!("only {field} lit field px for a three-digit ground — the panel did not draw");
            }
            Ok(format!("ground pattern drew itself ({field} lit field px)"))
        }
        _ => Ok("no display-truth procedure for this class — skipped".into()),
    }
}

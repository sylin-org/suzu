//! Admission tests run before a device may stream (ADR-0003). Tests
//! restore temporary writes and verify display output through a J capture.
//!
//! Steps depend on device capabilities:
//! - handshake — `I` returns a descriptor
//! - keepalive-ack — `K` returns `OK`
//! - label-roundtrip — only when the descriptor exposes `label`:
//!   write a marker, read it back, restore the original
//! - display verification — draw a known pattern and capture it,
//!   capture via J, and assert the pixels:
//!   · matrix: a blue `completion` pattern that no other display state
//!   produces (the other states remain below blue channel 80 at the
//!   configured half-brightness limit)
//!   · OLED: a known three-digit metrics pattern lights the data area

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

/// `"1.2.3"` → [1, 2, 3]; anything unparseable is None.
fn parse_version(v: &str) -> Option<Vec<u64>> {
    let parts: Result<Vec<u64>, _> =
        v.split('.').map(|p| p.trim().parse::<u64>()).collect();
    parts.ok()
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

/// Run admission on an open session. `spec`/`zones` come from the
/// class manifest; a device without a decodable frame format skips the
/// display-verification step. `faceplate_versions` contains the installed and
/// declared versions. Devices with outdated faceplates cannot stream until the
/// faceplate is updated (ADR-0005).
pub fn run(
    serial: &mut Box<dyn SerialPort>,
    class: Option<&str>,
    spec: Option<&FrameSpec>,
    zones: &[(usize, usize, [u8; 3])],
    faceplate_versions: Option<(&str, Option<&str>)>,
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

    // Validate the faceplate version before the more expensive display check.
    if let Some((installed, declared)) = faceplate_versions {
        let Some(declared) = declared else {
            fail(
                &mut report,
                "faceplate-version",
                format!("faceplate {installed} is not declared for this class; install a declared faceplate"),
            );
            return report;
        };
        match parse_version(declared) {
            None => report.step(
                "faceplate-version",
                true,
                format!("declared version `{declared}` unreadable — not asserted"),
            ),
            Some(declared_v) => {
                let installed_parsed = parse_version(installed);
                let current = installed_parsed
                    .as_ref()
                    .is_some_and(|version| version >= &declared_v);
                if current {
                    report.step("faceplate-version", true, format!("faceplate {installed} is current"));
                } else {
                    let installed_text = installed_parsed
                        .map(|v| v.iter().map(|n| n.to_string()).collect::<Vec<_>>().join("."))
                        .unwrap_or_else(|| "unversioned".to_string());
                    fail(
                        &mut report,
                        "faceplate-version",
                        format!("faceplate {installed_text} is older than declared version {declared}; update it before streaming"),
                    );
                    return report;
                }
            }
        }
    }

    // Verify keepalive acknowledgement.
    let _ = write_line(serial, "K");
    match read_line_matching(serial, HANDSHAKE_SECS, |l| l == "OK" || l.starts_with("OK,")) {
        Some(_) => report.step("keepalive-ack", true, "K answered OK".into()),
        None => {
            fail(&mut report, "keepalive-ack", "K answered nothing".into());
            return report;
        }
    }

    // ── label roundtrip (only when the faceplate declares a label) ──
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

    // Verify that a known pattern is visible in the captured frame.
    let display_result = display_check(serial, class, spec, zones);
    match display_result {
        Ok(detail) => report.step("display-check", true, detail),
        Err(e) => {
            fail(&mut report, "display-check", format!("{e}"));
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

fn display_check(
    serial: &mut Box<dyn SerialPort>,
    class: Option<&str>,
    spec: Option<&FrameSpec>,
    zones: &[(usize, usize, [u8; 3])],
) -> Result<String> {
    let Some(spec) = spec else {
        return Ok("class declares no frame format; display check skipped".into());
    };
    match class {
        // Render a completion event and confirm its distinctive blue pixels.
        Some("waveshare-rp2040-matrix") => {
            let _ = view_of(serial, spec, zones)?; // drain any stale frame
            crate::shot::dribble_line(serial, "R,completion.0,2,0,1,1,admission")?;
            std::thread::sleep(Duration::from_millis(300));
            let (w, _h, rgba) = view_of(serial, spec, zones)?;
            let blue = rgba.chunks_exact(4).filter(|p| p[0] < 60 && p[1] > 50 && p[2] > 80).count();
            let _ = crate::shot::dribble_line(serial, "X"); // clear the test pattern
            if blue == 0 {
                anyhow::bail!("no completion-blue pixel in the {w}px captured frame");
            }
            Ok(format!("completion pattern rendered ({blue} blue px)"))
        }
        // Send a known three-digit metrics pattern. The device must
        // acknowledge it and the capture must contain enough lit pixels.
        Some("esp8266-oled") => {
            crate::shot::dribble_line(serial, "G,report,88,77,66")?;
            match read_line_matching(serial, HANDSHAKE_SECS, |l| {
                l == "OK" || l.starts_with("OK,") || l.starts_with("ERR")
            }) {
                Some(l) if l.starts_with("ERR") => {
                    anyhow::bail!("the device rejected the metrics frame: {l}")
                }
                Some(_) => {}
                None => anyhow::bail!("the device did not acknowledge the metrics frame"),
            }
            std::thread::sleep(Duration::from_millis(400));
            let (_w, _h, rgba) = view_of(serial, spec, zones)?;
            let field = rgba.chunks_exact(4).filter(|p| p[2] > 150).count();
            // The view is 1:1 with the framebuffer: a three-digit
            // The pattern lights about 1040 pixels; idle content
            // ~40 — 500 separates them by an order of magnitude
            // either way.
            if field < 500 {
                anyhow::bail!("only {field} data-area pixels lit for the test pattern; the panel did not draw it");
            }
            Ok(format!("metrics pattern acknowledged and rendered ({field} lit data-area pixels)"))
        }
        _ => Ok("class has no display-check procedure; skipped".into()),
    }
}

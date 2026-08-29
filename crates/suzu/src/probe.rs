//! The identification probe — the contract handshake, tool-side.
//!
//! Ladder: passive window for the unsolicited `* HELLO` frame, then the
//! `I` probe with the 4-second deadline. A device that cannot answer in
//! 4 seconds is not a suzu companion — it is a serial port that happens
//! to be attached.

use anyhow::Result;
use serde_json::Value;
use serialport::SerialPort;
use std::io::ErrorKind;
use std::time::{Duration, Instant};

pub const HANDSHAKE_DEADLINE: Duration = Duration::from_secs(4);
const _: () = assert!(
    HANDSHAKE_DEADLINE.as_secs() == 4,
    "READ_DEADLINE changed — review handshake latency assumptions \
     (ESP boot: ~2.5 s; identity emit: ~200 ms; USB hiccups: ~1.3 s)"
);

/// Line framing over a serial port: `\n`-delimited, incomplete tails
/// buffered, overflow-safe by construction.
pub struct Lines {
    buf: Vec<u8>,
}

impl Lines {
    pub fn new() -> Self {
        Self { buf: Vec::new() }
    }

    /// One read attempt (paced by the port timeout); returns every
    /// complete line that resulted.
    pub fn poll(&mut self, port: &mut Box<dyn SerialPort>) -> Result<Vec<String>> {
        let mut scratch = [0u8; 512];
        match port.read(&mut scratch) {
            Ok(0) => {}
            Ok(n) => self.buf.extend_from_slice(&scratch[..n]),
            Err(ref e) if e.kind() == ErrorKind::TimedOut => {}
            Err(e) => return Err(e.into()),
        }
        if self.buf.len() > 64 * 1024 {
            self.buf.clear();
        }
        Ok(self.drain())
    }

    fn drain(&mut self) -> Vec<String> {
        let mut out = Vec::new();
        while let Some(pos) = self.buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = self.buf.drain(..=pos).collect();
            let body = &line[..line.len() - 1]; // drop '\n'
            let s = String::from_utf8_lossy(body).trim_end().to_string();
            if !s.is_empty() {
                out.push(s);
            }
        }
        out
    }
}

/// The full ladder transcript — everything the detective needs.
/// Boot noise, timeouts, and line counts are evidence, not garbage.
#[derive(Debug, Default)]
pub struct Transcript {
    pub hello: bool,
    pub identity: Option<Value>,
    pub identity_raw: Option<String>,
    pub identity_after_ms: Option<u128>,
    pub legacy_line: Option<String>,
    /// Every line seen during the ladder, truncated.
    pub lines: Vec<String>,
    pub error: Option<String>,
}

pub enum Outcome {
    /// A JSON descriptor answered. `hello` = it spoke first.
    Suzu { json: Value, hello: bool },
    /// Pre-suzu firefly CSV identity — ancestor firmware.
    LegacyFirefly { line: String },
    /// No identity response within the deadline.
    Silent,
}

fn strip_prefixes(line: &str) -> &str {
    let l = line.trim();
    let l = l.strip_prefix("* HELLO,").unwrap_or(l);
    let l = l.strip_prefix("OK,").unwrap_or(l);
    l.trim()
}

/// suzu-t: a session frame may carry a trailing `*hh` xor checksum —
/// the companion is required to send one on `OK,{…}` replies, so the
/// host must accept it. Without this, every honest identity reply
/// fails to parse as bare JSON and the probe calls a live face
/// "fresh firmware" (proven on the bench, 2026-08-28).
fn strip_checksum(line: &str) -> &str {
    let l = line.trim();
    if l.len() >= 3 {
        let (body, sum) = l.split_at(l.len() - 2);
        if body.ends_with('*') && sum.bytes().all(|b| b.is_ascii_hexdigit()) {
            return &body[..body.len() - 1];
        }
    }
    l
}

pub fn try_identity(line: &str) -> Option<Value> {
    let l = strip_checksum(line);
    let body = strip_prefixes(l);
    if body.starts_with('{') {
        return serde_json::from_str(body).ok();
    }
    // Boot noise can glue itself onto the response line (no newline
    // between junk and `OK,{…}`) — parse from the first `{` to the
    // last `}` and let serde judge.
    let start = l.find('{')?;
    let end = l.rfind('}')? + 1;
    if end <= start {
        return None;
    }
    serde_json::from_str(&l[start..end]).ok()
}

fn is_legacy(line: &str) -> bool {
    // Tolerate glued boot noise: search, don't anchor.
    line.contains("firefly-") || line.contains("firefly,")
}

pub fn probe_transcript(port_name: &str) -> Transcript {
    fn handle(t: &mut Transcript, line: String) -> bool {
        if t.lines.len() < 24 {
            t.lines.push(line.chars().take(512).collect());
        }
        if line.contains("HELLO") {
            t.hello = true;
        }
        if let Some(json) = try_identity(&line) {
            t.identity = Some(json);
            t.identity_raw = Some(line);
            return true;
        }
        if is_legacy(&line) {
            t.legacy_line = Some(line);
            return true;
        }
        false
    }

    let mut t = Transcript::default();
    let mut port = match serialport::new(port_name, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
    {
        Ok(p) => p,
        Err(e) => {
            t.error = Some(format!("{port_name}: {e}"));
            return t;
        }
    };
    let mut lines = Lines::new();
    let started = Instant::now();

    // 0. Boot wait — opening the port can hard-reset the board
    // (harvest constant: 2.5 s). Probing mid-boot loses the `I`.
    std::thread::sleep(Duration::from_millis(2500));

    // 0.5 Loving recovery — a previous session may have left the board
    // at a REPL prompt **with a giant unterminated edit line** (killed
    // pushes wedged exactly so: every new byte gets typed into the line
    // and re-echoed). Recovery order matters:
    //   Ctrl-C ×2  aborts the wedged line  -> clean prompt
    //   Ctrl-B     ensures friendly mode  -> ">>>"
    //   Ctrl-D     soft reboot            -> boot.py + main.py run again
    // (A soft reboot WITHOUT the abort just types "2,4" into the line.)
    let _ = port.write_all(b"\x03\x03");
    let _ = port.flush();
    std::thread::sleep(Duration::from_millis(300));
    let _ = port.write_all(b"\x02\x04");
    let _ = port.flush();
    std::thread::sleep(Duration::from_millis(2500));

    // 1. Passive window — some firmware speaks first on boot. (The
    // deadline starts NOW: anchoring it to `started` made this window
    // unrunnable — it sat entirely inside the 5 s recovery preamble.)
    let deadline = Instant::now() + Duration::from_millis(2000);
    while Instant::now() < deadline {
        match lines.poll(&mut port) {
            Ok(seen) => {
                for line in seen {
                    if handle(&mut t, line) {
                        t.identity_after_ms = Some(started.elapsed().as_millis());
                        return t;
                    }
                }
            }
            Err(e) => {
                t.error = Some(e.to_string());
                return t;
            }
        }
    }

    // 2. The handshake: write `I`, expect JSON within the deadline —
    // asking twice, because a boot race can swallow the first ask.
    use std::io::Write;
    for attempt in 0..2 {
        if let Err(e) = port.write_all(b"I\n").and_then(|_| port.flush()) {
            t.error = Some(e.to_string());
            return t;
        }
        let deadline = Instant::now() + HANDSHAKE_DEADLINE;
        while Instant::now() < deadline {
            match lines.poll(&mut port) {
                Ok(seen) => {
                    for line in seen {
                        if handle(&mut t, line) {
                            t.identity_after_ms = Some(started.elapsed().as_millis());
                            return t;
                        }
                    }
                }
                Err(e) => {
                    t.error = Some(e.to_string());
                    return t;
                }
            }
        }
        if attempt == 0 {
            let _ = port.write_all(b"I\n");
            let _ = port.flush();
        }
    }
    t
}

/// The ladder, reduced to the three-way outcome.
pub fn probe(port_name: &str) -> Result<Outcome> {
    let t = probe_transcript(port_name);
    if let Some(e) = &t.error {
        return Err(anyhow::anyhow!("{e}"));
    }
    if let Some(json) = &t.identity {
        return Ok(Outcome::Suzu {
            json: json.clone(),
            hello: t.hello,
        });
    }
    if let Some(line) = &t.legacy_line {
        return Ok(Outcome::LegacyFirefly { line: line.clone() });
    }
    Ok(Outcome::Silent)
}

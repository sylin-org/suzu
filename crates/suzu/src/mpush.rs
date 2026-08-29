//! The REPL push engine — file transfer over the MicroPython serial REPL.
//!
//! Used by the install/upgrade/factory-reset procedures for boards that
//! already run MicroPython (the gentle path — no bootloader). Protocol:
//! interrupt the running app, enter raw REPL, exec lines with Ctrl-D,
//! chunked escaped writes (the harvest's 512 B ceiling, tightened to
//! 256 B), binary-safe reads via hexlify, verify after every write.

use anyhow::{bail, Result};
use serialport::SerialPort;
use std::io::{ErrorKind, Write};
use std::time::{Duration, Instant};

/// Boot wait after opening the port — ESP auto-resets on open
/// (harvest constant: 2.5 s).
const BOOT_WAIT: Duration = Duration::from_millis(2500);
const EXEC_TIMEOUT: Duration = Duration::from_secs(15);
const CHUNK: usize = 256;

pub struct Repl {
    port: Box<dyn SerialPort>,
    buf: Vec<u8>,
    /// True while the device is in raw REPL.
    raw: bool,
}

fn find(buf: &[u8], marker: &[u8]) -> Option<usize> {
    if marker.len() > buf.len() {
        return None;
    }
    (0..=buf.len() - marker.len()).find(|&i| &buf[i..i + marker.len()] == marker)
}

/// Encode bytes as a Python `b'…'` literal (safe inside exec lines).
fn py_bytes_literal(data: &[u8]) -> String {
    let mut s = String::from("b'");
    for &b in data {
        match b {
            b'\\' => s.push_str("\\\\"),
            b'\'' => s.push_str("\\'"),
            b'\r' => s.push_str("\\r"),
            b'\n' => s.push_str("\\n"),
            b'\t' => s.push_str("\\t"),
            0x20..=0x7e => s.push(b as char),
            _ => s.push_str(&format!("\\x{b:02x}")),
        }
    }
    s.push('\'');
    s
}

impl Repl {
    /// Open the port and quiet whatever is running on it.
    pub fn open(port_name: &str) -> Result<Self> {
        let mut port = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(200))
            .open()
            .map_err(|e| anyhow::anyhow!("{port_name}: {e}"))?;
        // CircuitPython gates its console on DTR — without it a live
        // board answers in silence (see resident/devices.rs).
        let _ = port.write_data_terminal_ready(true);
        std::thread::sleep(BOOT_WAIT);
        // Interrupt any running application (Ctrl-C ×2) and drain the
        // boot/banner noise.
        let _ = port.write_all(b"\r\x03\x03");
        let _ = port.flush();
        let mut repl = Self {
            port,
            buf: Vec::new(),
            raw: false,
        };
        repl.drain(Duration::from_millis(700));
        Ok(repl)
    }

    fn drain(&mut self, for_: Duration) {
        let deadline = Instant::now() + for_;
        let mut scratch = [0u8; 512];
        while Instant::now() < deadline {
            match self.port.read(&mut scratch) {
                Ok(0) => {}
                Ok(_) => {}
                Err(ref e) if e.kind() == ErrorKind::TimedOut => {}
                Err(_) => {}
            }
        }
        self.buf.clear();
    }

    fn read_until(&mut self, marker: &[u8], timeout: Duration) -> Result<String> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(pos) = find(&self.buf, marker) {
                let text = String::from_utf8_lossy(&self.buf[..pos]).to_string();
                self.buf.drain(..pos + marker.len());
                return Ok(text);
            }
            if Instant::now() > deadline {
                bail!(
                    "serial timeout waiting for {:?}",
                    String::from_utf8_lossy(marker)
                );
            }
            let mut scratch = [0u8; 512];
            match self.port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => self.buf.extend_from_slice(&scratch[..n]),
                Err(ref e) if e.kind() == ErrorKind::TimedOut => {}
                Err(e) => bail!("{e}"),
            }
        }
    }

    /// Interrupt any running application and return to the friendly
    /// prompt.
    pub fn interrupt(&mut self) {
        let _ = self.port.write_all(b"\r\x03\x03");
        let _ = self.port.flush();
        self.drain(Duration::from_millis(700));
        self.raw = false;
    }

    /// Enter raw REPL (Ctrl-A) — interrupting first, so a running app
    /// cannot swallow the mode switch.
    pub fn enter_raw(&mut self) -> Result<()> {
        self.interrupt();
        self.port.write_all(b"\x01")?;
        self.port.flush()?;
        self.read_until(b"raw REPL", Duration::from_secs(3))?;
        self.read_until(b">", Duration::from_secs(3))?;
        self.raw = true;
        Ok(())
    }

    /// Leave raw REPL (Ctrl-B) back to the friendly prompt.
    pub fn exit_raw(&mut self) -> Result<()> {
        if self.raw {
            self.port.write_all(b"\x02")?;
            self.port.flush()?;
            self.drain(Duration::from_millis(500));
            self.raw = false;
        }
        Ok(())
    }

    /// Execute one statement in raw REPL; returns its stdout. Fails on
    /// tracebacks — the device's error is the error.
    pub fn exec(&mut self, code: &str) -> Result<String> {
        if !self.raw {
            self.enter_raw()?;
        }
        self.buf.clear(); // stale response fragments must never glue onto the next reply
        if let Err(e) = self
            .port
            .write_all(code.as_bytes())
            .and_then(|_| self.port.write_all(b"\x04"))
            .and_then(|_| self.port.flush())
        {
            self.raw = false;
            bail!("{e}");
        }
        let out = match self.read_until(b"\x04", EXEC_TIMEOUT) {
            Ok(out) => out,
            Err(e) => {
                self.raw = false; // framing unknown after a timeout
                return Err(e);
            }
        };
        self.read_until(b">", Duration::from_secs(5))?;
        if out.contains("Traceback") {
            bail!("device raised:\n{}", out.trim());
        }
        Ok(out)
    }

    /// Binary-safe file read: hexlify on-device, decode host-side.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        self.enter_raw()?;
        let code = format!(
            "import ubinascii; print(ubinascii.hexlify(open('{name}','rb').read()))"
        );
        self.port.write_all(code.as_bytes())?;
        self.port.write_all(b"\x04")?;
        self.port.flush()?;
        let out = self.read_until(b"\x04", EXEC_TIMEOUT)?;
        self.read_until(b">", Duration::from_secs(5))?;
        self.exit_raw()?;
        if out.contains("Traceback") {
            bail!("read failed for {name}");
        }
        let hex: String = out.chars().filter(|c| c.is_ascii_hexdigit()).collect();
        (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).map_err(Into::into))
            .collect()
    }

    /// Chunked escaped write + read-back verification.
    pub fn write_file(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.exec(&format!("f = open('{name}','wb')"))?;
        for chunk in data.chunks(CHUNK) {
            self.exec(&format!("f.write({})", py_bytes_literal(chunk)))?;
        }
        self.exec("f.close()")?;
        self.exit_raw()?;
        // Verify after write — the read-back is the loving part.
        let back = self.read_file(name)?;
        if back != data {
            bail!("write verification failed for {name} ({} vs {} bytes)", back.len(), data.len());
        }
        Ok(())
    }

    pub fn list_files(&mut self) -> Result<Vec<String>> {
        let out = self.exec("import os; print(os.listdir())")?;
        let inner = out
            .trim()
            .trim_start_matches('[')
            .trim_end_matches(']')
            .to_string();
        Ok(inner
            .split(',')
            .map(|s| s.trim().trim_matches('\'').trim_matches('"').to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    pub fn remove_file(&mut self, name: &str) -> Result<()> {
        self.exec(&format!("import os; os.remove('{name}')"))
            .map(|_| ())
    }

    /// Soft reboot: boot.py → main.py run again. The port closes with
    /// the Repl; probe fresh afterwards.
    pub fn soft_reboot(mut self) -> Result<()> {
        self.exit_raw()?;
        self.port.write_all(b"\x04")?;
        self.port.flush()?;
        self.drain(Duration::from_millis(2500));
        Ok(())
    }
}

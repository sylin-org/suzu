//! Native raw-REPL engine for device provisioning.
//!
//! A MicroPython board not yet speaking suzu/1 can only be reached
//! through its interpreter's console: interrupt the app, enter raw
//! mode, evaluate one-liners, and write files as base64 chunk lines
//! sized for the UART RX FIFO. This module is the Rust port of
//! the reference procedure that lived in `scripts/push_firmware.py` —
//! with equivalent framing, verification, and error handling, so provisioning
//! requires only this binary.
//!
//! Protocol requirements:
//! - raw mode is accepted only when the device returns the `raw REPL`
//!   banner); a session without the banner is not a session;
//! - the end marker is the PAIR `\x04>`, never a bare `\x04`;
//! - framing failures abort loudly — blind retries can double-write;
//! - every file is read back and verified after its write;
//! - everything on a device is backed up before the first write.

use anyhow::{bail, Result};
use serialport::{ClearBuffer, SerialPort};
use std::io::Write;
use std::time::{Duration, Instant};

const CHUNK: usize = 192; // base64 chars per chunk-line (144 B binary):
                          // short lines survive the ESP8266's UART RX FIFO
const READ_SLICE: usize = 384; // hexlify doubles it on-device: 384 -> 768, safe
const BOOT_WAIT: u64 = 2500; // the ESP auto-resets when the port opens

pub struct Repl {
    port: Box<dyn SerialPort>,
    raw: bool,
}

impl Repl {
    /// Open, wait out the boot the DTR pulse causes, and prove the
    /// session sane before anything is read or written.
    pub fn open(port_name: &str) -> Result<Self> {
        let mut port: Box<dyn SerialPort> = serialport::new(port_name, 115_200)
            .timeout(Duration::from_millis(300))
            .open()
            .map_err(|e| anyhow::anyhow!("{port_name}: {e}"))?;
        let _ = port.write_data_terminal_ready(true);
        std::thread::sleep(Duration::from_millis(BOOT_WAIT));
        let mut repl = Self { port, raw: false };
        repl.ensure_raw()?;
        // The sanity round-trip: the same path every later step needs,
        // proven working BEFORE anything is touched.
        let out = repl.exec("print('suzu-ok')")?;
        if !out.windows(7).any(|w| w == b"suzu-ok") {
            bail!("REPL answered but not sanely: {:?}", &out[..out.len().min(80)]);
        }
        // The previously running application may leave the heap fragmented; a
        // collect here is the difference between a parse fitting or not.
        repl.exec("import gc; gc.collect()")?;
        Ok(repl)
    }

    /// Enter raw mode and believe it only when the device says so.
    fn ensure_raw(&mut self) -> Result<()> {
        for attempt in 1..=3 {
            let _ = self.port.write_all(b"\r\x03\x03"); // interrupt any app
            let _ = self.port.flush();
            std::thread::sleep(Duration::from_millis(700));
            self.drain(0.5); // Allow time for the application exit response.
            let _ = self.port.write_all(b"\x02"); // Ctrl-B: friendly, known state
            let _ = self.port.flush();
            std::thread::sleep(Duration::from_millis(300));
            self.drain(0.3);
            let _ = self.port.write_all(b"\x01"); // Ctrl-A: raw mode
            let _ = self.port.flush();
            std::thread::sleep(Duration::from_millis(300));
            let banner = self.drain(0.5);
            if banner.windows(8).any(|w| w == b"raw REPL") {
                self.raw = true;
                println!("  raw REPL confirmed (attempt {attempt})");
                return Ok(());
            }
        }
        bail!("could not confirm raw REPL — device untouched")
    }

    /// Read whatever arrives for `secs` seconds.
    fn drain(&mut self, secs: f32) -> Vec<u8> {
        let end = Instant::now() + Duration::from_secs_f32(secs);
        let mut buf = Vec::new();
        while Instant::now() < end {
            let waiting = self.port.bytes_to_read().unwrap_or(0) as usize;
            let mut scratch = vec![0u8; waiting.max(1)];
            match self.port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => buf.extend_from_slice(&scratch[..n]),
                Err(_) => {}
            }
        }
        buf
    }

    /// One raw-REPL round trip. Framing is verified, never assumed;
    /// a lost frame aborts loudly (blind retries can double-write).
    pub fn exec(&mut self, code: &str) -> Result<Vec<u8>> {
        if !self.raw {
            self.ensure_raw()?;
        }
        self.port.write_all(code.as_bytes())?;
        self.port.write_all(b"\x04")?;
        self.port.flush()?;
        // The end marker is the PAIR `\x04>` — a bare `\x04` check only
        // passes when a read happens to split between the two bytes.
        let mut out = Vec::new();
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end && !out.ends_with(b"\x04>") {
            let mut scratch = [0u8; 512];
            match self.port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => out.extend_from_slice(&scratch[..n]),
                Err(_) => {}
            }
        }
        if !out.ends_with(b"\x04>") {
            self.raw = false;
            bail!(
                "no end-of-reply marker — framing unknown, aborting \
                 (device untouched; re-run re-verifies every file)"
            );
        }
        if out.windows("Traceback".len()).any(|w| w == b"Traceback") {
            bail!(
                "device raised:\n{}",
                String::from_utf8_lossy(&out)
            );
        }
        Ok(out)
    }

    /// Hold until the friendly prompt actually answers — the first
    /// write line must never race the post-interrupt transition.
    fn sync_prompt(&mut self) -> bool {
        let _ = self.port.write_all(b"\r\n");
        let _ = self.port.flush();
        let mut got = Vec::new();
        let end = Instant::now() + Duration::from_secs(3);
        while Instant::now() < end {
            let mut scratch = [0u8; 64];
            match self.port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => got.extend_from_slice(&scratch[..n]),
                Err(_) => {}
            }
            let last_nonws = got.iter().rposition(|b| *b > b' ').map(|i| i + 1).unwrap_or(0);
            if got[..last_nonws].ends_with(b">>>") {
                return true;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        false
    }

    /// One line to the friendly prompt, dribbled 16 chars at a time
    /// (the ESP8266's UART RX FIFO overruns a 200+ char burst; a
    /// truncated line with an open quote swallows the session
    /// silently), waited out to the `>>> ` prompt.
    fn friendly_line(&mut self, s: &str) -> Result<()> {
        let mut payload = s.as_bytes().to_vec();
        payload.extend_from_slice(b"\r\n");
        for chunk in payload.chunks(16) {
            self.port.write_all(chunk)?;
            self.port.flush()?;
            std::thread::sleep(Duration::from_millis(4));
        }
        let mut got: Vec<u8> = Vec::new();
        let end = Instant::now() + Duration::from_secs(5);
        while Instant::now() < end {
            let waiting = self.port.bytes_to_read().unwrap_or(0) as usize;
            let mut scratch = vec![0u8; waiting.max(1)];
            match self.port.read(&mut scratch) {
                Ok(0) => {}
                Ok(n) => got.extend_from_slice(&scratch[..n]),
                Err(_) => {}
            }
            let tail = got.len().saturating_sub(8);
            if got[tail..].windows(3).any(|w| w == b">>>") {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        // unwind a stuck line
        let _ = self.port.write_all(b"\r\x03\x03");
        let _ = self.port.flush();
        self.drain(0.4);
        bail!(
            "line never reached the prompt: {:?}... reply tail: {:?}",
            &s[..s.len().min(60)],
            String::from_utf8_lossy(&got)
        )
    }

    /// The hardened write: interrupt, friendly prompt, base64 chunk
    /// lines, then read-back verify in raw mode — never trust a blind
    /// write, and never whole-file hexlify (it doubles the bytes
    /// on-device and blows the heap floor).
    pub fn write_file(&mut self, name: &str, data: &[u8]) -> Result<()> {
        self.exec("import gc; gc.collect()")?;
        let _ = self.port.write_all(b"\r\x03\x03"); // interrupt whatever runs
        let _ = self.port.flush();
        std::thread::sleep(Duration::from_millis(700));
        self.drain(0.5);
        let _ = self.port.write_all(b"\x02"); // Ctrl-B: friendly, deliberately
        let _ = self.port.flush();
        std::thread::sleep(Duration::from_millis(400));
        self.drain(0.4);
        self.raw = false;
        if !self.sync_prompt() {
            bail!("friendly prompt not answering — device untouched");
        }

        self.friendly_line(&format!("f = open('{name}','wb')"))?;
        self.friendly_line("import ubinascii")?;
        let b64 = crate::shot::encode_b64(data);
        for chunk in b64.as_bytes().chunks(CHUNK) {
            let chunk = std::str::from_utf8(chunk)?;
            self.friendly_line(&format!("f.write(ubinascii.a2b_base64('{chunk}'))"))?;
        }
        self.friendly_line("f.close()")?;
        self.friendly_line(&format!("import os; print('SIZE:', os.stat('{name}')[6])"))?;
        std::thread::sleep(Duration::from_millis(200));

        // Read-back verify — sliced, on the raw path again.
        self.ensure_raw()?;
        let got = self.read_file(name)?;
        if got != data {
            bail!("verify failed for {name}");
        }
        println!("  OK {name} ({} bytes verified)", data.len());
        Ok(())
    }

    pub fn list_files(&mut self) -> Result<Vec<String>> {
        let out = self.exec("import os; print(os.listdir())")?;
        let text = String::from_utf8_lossy(&out);
        // Extract the bracket section — a late exit-reply can glue an
        // `OK` (or boot noise) onto the front of the real answer.
        let (Some(a), Some(b)) = (text.find('['), text.rfind(']')) else {
            bail!("unparseable list_files reply: {:?}", &text[..text.len().min(120)]);
        };
        if b < a {
            bail!("unparseable list_files reply: {:?}", &text[..text.len().min(120)]);
        }
        Ok(text[a + 1..b]
            .split(',')
            .map(|s| s.trim().trim_matches(['\'', '"']).to_string())
            .filter(|s| !s.is_empty())
            .collect())
    }

    /// Read a device file back in small slices — hexlify doubles the
    /// bytes on-device, so the slice obeys the 2 KB heap floor.
    pub fn read_file(&mut self, name: &str) -> Result<Vec<u8>> {
        let mut data = Vec::new();
        self.exec(&format!("f = open('{name}','rb')"))?;
        loop {
            let reply = self.exec(&format!(
                "import ubinascii; print(ubinascii.hexlify(f.read({READ_SLICE})))"
            ))?;
            let text = String::from_utf8_lossy(&reply);
            let Some(start) = text.find("b'") else {
                bail!("could not parse backup slice: {:?}", &text[..text.len().min(120)]);
            };
            let Some(end) = text[start + 2..].find('\'') else {
                bail!("could not parse backup slice: {:?}", &text[..text.len().min(120)]);
            };
            let hex = &text[start + 2..start + 2 + end];
            let chunk = decode_hex(hex)?;
            if chunk.is_empty() {
                break;
            }
            data.extend_from_slice(&chunk);
        }
        self.exec("f.close()")?;
        Ok(data)
    }

    /// Dump every existing file before any write — the procedure's
    /// license to touch a working device.
    pub fn backup_files(&mut self, names: &[String], port_tag: &str) -> Result<()> {
        if names.is_empty() {
            println!("nothing to back up (fresh filesystem)");
            return Ok(());
        }
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let safe_tag: String = port_tag
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
            .collect();
        let dest = crate::paths::backups_dir().join(format!("{safe_tag}-{stamp}"));
        std::fs::create_dir_all(&dest)?;
        for name in names {
            match self.read_file(name) {
                Ok(data) => {
                    std::fs::write(dest.join(name), &data)?;
                    println!("  backed up {name} ({} bytes)", data.len());
                }
                Err(e) => println!("  backup skipped {name} ({e:#})"),
            }
        }
        println!("backup at {}", dest.display());
        Ok(())
    }

    pub fn remove_file(&mut self, name: &str) -> Result<()> {
        self.exec(&format!("import os; os.remove('{name}')"))?;
        Ok(())
    }

    /// Ctrl-B to the friendly prompt, then Ctrl-D: the board re-runs
    /// main.py; after this call the device restarts independently.
    pub fn soft_reboot(mut self) -> Result<()> {
        self.raw = false;
        let _ = self.port.write_all(b"\x02");
        let _ = self.port.flush();
        std::thread::sleep(Duration::from_millis(400));
        let _ = self.port.clear(ClearBuffer::Input);
        let _ = self.port.write_all(b"\x04");
        let _ = self.port.flush();
        std::thread::sleep(Duration::from_millis(2500));
        Ok(())
    }
}

fn decode_hex(s: &str) -> Result<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        bail!("odd hex length in slice");
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            u8::from_str_radix(&s[i..i + 2], 16)
                .map_err(|e| anyhow::anyhow!("bad hex {e}"))
        })
        .collect()
}

//! `suzu prepare` — the adoption front door.
//!
//! Lists every plugged candidate (serial faces and CircuitPython
//! drives), shows its honest state (suzu / pre-suzu firefly / blank),
//! offers the class's faceplates, and installs by the class's reliable
//! path: CircuitPython drives get file copies with read-back verify;
//! REPL faces get the proven push.

use crate::catalog::Catalog;
use crate::probe;
use anyhow::bail;
use serde::Deserialize;
use std::io::{self, Write};

#[derive(Debug, Deserialize)]
struct FaceplateDecl {
    name: String,
    class: String,
    #[serde(default)]
    status: Option<String>,
}

/// The faceplates declared in the repo, keyed by class id.
fn faceplates_for(class: &str) -> Vec<FaceplateDecl> {
    let mut out = Vec::new();
    let root = std::path::Path::new("faceplates");
    let Ok(entries) = std::fs::read_dir(root) else {
        return out;
    };
    for dir in entries.flatten() {
        let decl = dir.path().join("faceplate.yaml");
        let Ok(text) = std::fs::read_to_string(&decl) else {
            continue;
        };
        if let Ok(d) = serde_yaml::from_str::<FaceplateDecl>(&text) {
            if d.class == class {
                out.push(d);
            }
        }
    }
    out
}

pub(crate) fn mint_v7() -> String {
    // GUIDv7: 48-bit ms timestamp, version 7, variant bits. Uniqueness
    // on a bench comes from the millisecond clock; the rest is shape.
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0);
    let mut b = [0u8; 16];
    b[0..6].copy_from_slice(&ms.to_be_bytes()[2..8]);
    b[6] = 0x70 | (0x0F & (std::process::id() as u8));
    b[8] = 0x80 | ((ms as u8) & 0x3F);
    // entropy for the low bits: process id + a monotonic nanos mix
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0) as u64;
    let mut seed = (std::process::id() as u64) ^ (nanos << 8) ^ 0x9E37_79B9;
    for i in 6..16 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        b[i] = (seed >> 33) as u8;
    }
    // re-apply version/variant after the fill
    b[6] = 0x70 | (0x0F & (std::process::id() as u8));
    b[8] = 0x80 | ((b[8] >> 2) & 0x3F);
    let hex: String = b.iter().map(|x| format!("{x:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// Candidate kinds, in the Keeper's vocabulary.
enum State {
    Suzu { version: String, faceplate: Option<String> },
    Firefly { version: Option<String> },
    Drive { cpy: bool },
    Blank,
}

struct Candidate {
    name: String,
    class: Option<String>,
    device_id: Option<String>,
    state: State,
}

impl Candidate {
    fn line(&self, idx: usize) -> String {
        let kind = self.class.clone().unwrap_or_else(|| "unknown".into());
        match &self.state {
            State::Suzu { version, faceplate } => format!(
                "[{idx}] {} - suzu/{version} - {kind}{}",
                self.name,
                faceplate
                    .as_ref()
                    .map(|f| format!(" ({f})"))
                    .unwrap_or_default()
            ),
            State::Firefly { version } => format!(
                "[{idx}] {} - Firefly/{} - {kind}",
                self.name,
                version.clone().unwrap_or_else(|| "?".into())
            ),
            State::Drive { cpy } => format!(
                "[{idx}] {} - {} - {kind}",
                self.name,
                if *cpy { "CircuitPython" } else { "drive" }
            ),
            State::Blank => format!("[{idx}] {} - BLANK - {kind}", self.name),
        }
    }
}

fn serial_candidates(catalog: &Catalog) -> Vec<Candidate> {
    let mut out = Vec::new();
    for e in crate::enumerate() {
        let Some(usb) = &e.usb else { continue };
        let t = probe::probe_transcript(&e.name);
        if let Some(err) = &t.error {
            println!("  {} - skipped ({err})", e.name);
            continue;
        }
        let (class, state) = match &t.identity {
            Some(json) => {
                let version = json
                    .get("version")
                    .and_then(|v| v.as_str())
                    .unwrap_or("?")
                    .to_string();
                let proto = json.get("proto").and_then(|v| v.as_str()).unwrap_or("");
                let class_id = catalog
                    .class_by_vidpid(usb.vid, usb.pid)
                    .map(|c| c.id.clone())
                    .unwrap_or_default();
                if proto == "suzu/1" {
                    let faceplate = json
                        .get("faceplate")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    (
                        class_id,
                        State::Suzu { version, faceplate },
                    )
                } else {
                    (class_id, State::Firefly { version: Some(version) })
                }
            }
            None => {
                let class_id = catalog
                    .class_by_vidpid(usb.vid, usb.pid)
                    .map(|c| c.id.clone())
                    .unwrap_or_default();
                (class_id, State::Blank)
            }
        };
        let device_id = t
            .identity
            .as_ref()
            .and_then(|j| j.get("device_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
        out.push(Candidate {
            name: e.name.clone(),
            class: Some(class),
            device_id,
            state,
        });
    }
    out
}

/// CircuitPython drives: a `boot_out.txt` on a mounted letter.
fn drive_candidates() -> Vec<Candidate> {
    let mut out = Vec::new();
    for letter in 'A'..='Z' {
        let root = format!("{letter}:\\");
        if std::path::Path::new(&format!("{root}boot_out.txt")).exists() {
            out.push(Candidate {
                name: format!("{letter}:"),
                class: Some("rp2040-matrix".into()),
                device_id: None,
                state: State::Drive { cpy: true },
            });
        }
    }
    out
}

/// CircuitPython reloads (and briefly dismounts the drive) when its
/// CDC port is opened or a file changes — the install waits out the
/// remount and retries writes. Getting stuck here is how drives die.
fn wait_drive(drive: &str, secs: u64) -> anyhow::Result<()> {
    let marker = std::path::PathBuf::from(format!("{drive}/boot_out.txt"));
    let end = std::time::Instant::now() + std::time::Duration::from_secs(secs);
    while std::time::Instant::now() < end {
        if marker.exists() {
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_millis(300));
    }
    bail!("{drive} did not remount within {secs} s — replug the device and retry")
}

fn write_drive_file(drive: &str, name: &str, data: &[u8]) -> anyhow::Result<()> {
    let path = std::path::PathBuf::from(format!("{drive}/{name}"));
    for attempt in 1..=4 {
        match std::fs::write(&path, data) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 4 => {
                println!("  write {name} retry {attempt} ({e}) — waiting out the remount");
                std::thread::sleep(std::time::Duration::from_millis(900));
            }
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

fn install_rp2040(drive: &str, class: &str) -> anyhow::Result<()> {
    // CircuitPython dismounts the drive on every reload: writes race
    // the remount and fail with transient errors. Three whole passes
    // with settled waits beat clever per-op handling.
    let mut last_err: Option<anyhow::Error> = None;
    for attempt in 1..=3 {
        match install_rp2040_once(drive) {
            Ok(()) => return Ok(()),
            Err(e) => {
                println!("  install pass {attempt} failed: {e:#} — waiting for the remount");
                last_err = Some(e);
                std::thread::sleep(std::time::Duration::from_secs(3));
                wait_drive(drive, 10)?;
            }
        }
    }
    Err(last_err.unwrap_or_else(|| anyhow::anyhow!("install failed")))
}

#[allow(unused_variables)]
fn install_rp2040_once(drive: &str) -> anyhow::Result<()> {
    wait_drive(drive, 10)?;
    let src = std::path::Path::new("firmware/suzu-d/rp2040-matrix");
    // Rule zero: backup what the drive holds before writing.
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    // a drive letter brings a colon: invalid in a Windows path component
    let safe_drive = drive.trim_end_matches(':');
    let dest = std::path::PathBuf::from(format!("backups/{safe_drive}-{stamp}"));
    std::fs::create_dir_all(&dest)
        .map_err(|e| anyhow::anyhow!("create_dir_all {}: {e}", dest.display()))?;
    for name in ["code.py", "suzu.json", "zen-garden.json"] {
        let p = std::path::PathBuf::from(format!("{drive}/{name}"));
        println!("  checking {name}: exists={}", p.exists());
        if p.exists() {
            std::fs::copy(&p, dest.join(name))
                .map_err(|e| anyhow::anyhow!("backup copy {name}: {e}"))?;
            println!("  backed up {name}");
        }
    }

    // Identity: preserve the drive's device_id when present, else mint.
    let mut suzu: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(src.join("suzu.json"))
            .unwrap_or_else(|_| "{\"proto\":\"suzu/1\"}".into()),
    )?;
    let existing = std::path::PathBuf::from(format!("{drive}/suzu.json"));
    if existing.exists() {
        if let Ok(old) = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(existing).unwrap_or_default(),
        ) {
            if let Some(id) = old.get("device_id").and_then(|v| v.as_str()) {
                suzu["device_id"] = serde_json::Value::String(id.to_string());
                println!("  identity preserved: {id}");
            }
        }
    }
    if suzu.get("device_id").map(|v| v == "assigned at adoption (preserved from pre-suzu provisioning when present)").unwrap_or(false) || suzu.get("device_id").is_none() {
        let id = mint_v7();
        suzu["device_id"] = serde_json::Value::String(id.clone());
        println!("  identity minted: {id}");
    }

    // The class's suzu-d firmware ships with the tool.
    let code = std::fs::read(src.join("code.py"))
        .map_err(|e| anyhow::anyhow!("read firmware code.py: {e}"))?;
    write_drive_file(drive, "code.py", &code)?;
    let sj = serde_json::to_vec(&suzu)?;
    write_drive_file(drive, "suzu.json", &sj)?;

    // Read-back verify — never trust a blind write.
    for (name, written) in [("code.py", &code), ("suzu.json", &sj)] {
        let back = std::fs::read(std::path::PathBuf::from(format!("{drive}/{name}")))?;
        if back != *written {
            bail!("verify failed for {name}");
        }
        println!("  OK {name} ({} bytes verified)", back.len());
    }
    println!("  CircuitPython reloads automatically — the face starts on its own");
    Ok(())
}

fn install_esp8266(port: &str, device_id: Option<&str>) -> anyhow::Result<()> {
    // The proven installer, verbatim: backup-first, chunked writes,
    // per-file verify, soft reboot. A Rust port lands with the push
    // engine; until then the reliable path wins over the pretty one.
    println!("  pushing via scripts/push_firmware.py (proven path) ...");
    let id = device_id.unwrap_or("");
    let status = std::process::Command::new("python")
        .args(["scripts/push_firmware.py", port, id, "--fresh"])
        .status()
        .map_err(|e| anyhow::anyhow!("python not found: {e}"))?;
    if !status.success() {
        bail!("push reported failure — device untouched where possible");
    }
    Ok(())
}

pub fn run(catalog: &Catalog) -> anyhow::Result<()> {
    println!("suzu prepare — adopt a firefly");
    let mut candidates = drive_candidates();
    candidates.extend(serial_candidates(catalog));
    if candidates.is_empty() {
        println!("  no candidates — plug a firefly in (data cable, not charge-only)");
        return Ok(());
    }
    for (i, c) in candidates.iter().enumerate() {
        println!("  {}", c.line(i + 1));
    }
    if candidates.len() == 1 {
        println!("  (single candidate — selected)");
    }
    print!(
        "Select the device you want to prepare, or enter to exit [1,{},enter]: ",
        candidates.len()
    );
    io::stdout().flush()?;
    let mut line = String::new();
    io::stdin().read_line(&mut line)?;
    let pick: usize = line.trim().parse().unwrap_or(0);
    if pick == 0 || pick > candidates.len() {
        println!("exit.");
        return Ok(());
    }
    let cand = &candidates[pick - 1];
    let class = cand.class.clone().unwrap_or_default();

    let plates = faceplates_for(&class);
    let faceplate = if plates.is_empty() {
        println!("  no faceplates declared for {class} — the suzu-d firmware is the face");
        None
    } else if plates.len() == 1 {
        println!("  faceplate: {} (only one — selected)", plates[0].name);
        Some(plates[0].name.clone())
    } else {
        for (i, p) in plates.iter().enumerate() {
            println!("  [{}] {}", i + 1, p.name);
        }
        print!("Select the faceplate [1,{}]: ", plates.len());
        io::stdout().flush()?;
        let mut l = String::new();
        io::stdin().read_line(&mut l)?;
        let i: usize = l.trim().parse().unwrap_or(1);
        Some(plates.get(i.saturating_sub(1)).map(|p| p.name.clone()).unwrap_or_default())
    };

    print!(
        "Prepare {} with {}? [Y/n]: ",
        cand.name,
        faceplate.as_deref().unwrap_or("the suzu-d firmware")
    );
    io::stdout().flush()?;
    let mut l = String::new();
    io::stdin().read_line(&mut l)?;
    let l = l.trim().to_lowercase();
    if !(l.is_empty() || l == "y" || l == "yes") {
        println!("cancelled — nothing written");
        return Ok(());
    }

    match cand.class.as_deref() {
        Some("rp2040-matrix") | Some("waveshare-rp2040-matrix") => {
            // The serial candidate pairs with its CircuitPython drive.
            let drive = if cand.name.starts_with("COM") {
                drive_candidates()
                    .into_iter()
                    .next()
                    .map(|d| d.name)
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "no CircuitPython drive found — hold BOOTSEL while plugging to recover"
                        )
                    })?
            } else {
                cand.name.clone()
            };
            install_rp2040(&drive, &class)?;
        }
        Some("esp8266-oled-v2") => {
            install_esp8266(&cand.name, cand.device_id.as_deref())?;
        }
        other => bail!("no install path for {other:?} yet — the class needs a procedure"),
    }

    // Verify, then say so.
    println!("verifying ...");
    let t = probe::probe_transcript(&cand.name);
    match &t.identity {
        Some(json) => println!(
            "  verified: proto {} · version {} · device_id {}",
            json.get("proto").and_then(|v| v.as_str()).unwrap_or("?"),
            json.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
            json.get("device_id").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        None => println!("  no identity yet — the face may need a moment; run `suzu scan`"),
    }
    println!("prepare complete.");
    Ok(())
}

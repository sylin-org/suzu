//! Servicing — the install / restore procedures, as pipelines over a
//! device. Loving rules: backup before touching, verify after writing,
//! and never continue past a failed read-back.

use crate::mpush::Repl;
use anyhow::{bail, Result};
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

const FACEPLATE_MAIN: &str =
    include_str!("../../../faceplates/esp8266-oled-v2/portrait-numerals/main.py");

fn backup_dir(device_id: &str) -> PathBuf {
    PathBuf::from("backups").join(device_id)
}

pub fn identify(port: &str) -> Result<serde_json::Value> {
    let t = crate::probe::probe_transcript(port);
    if let Some(e) = &t.error {
        bail!("{e}");
    }
    t.identity
        .ok_or_else(|| anyhow::anyhow!("no identity response — the device must run MicroPython firmware first"))
}

/// The migration: backup → push suzu identity + faceplate → reboot →
/// verify the handshake answers suzu/1 with the same device_id.
pub fn migrate(port: &str) -> Result<String> {
    let identity = identify(port)?;
    let device_id = identity
        .get("device_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("device has no device_id — cannot preserve identity"))?
        .to_string();
    let proto = identity.get("proto").and_then(|v| v.as_str()).unwrap_or("");
    if proto == "suzu/1" {
        return Ok("already suzu/1 — nothing to do".into());
    }

    println!("  [1/4] opening REPL on {port} …");
    let mut repl = Repl::open(port)?;

    println!("  [2/4] backing up current firmware …");
    let dir = backup_dir(&device_id);
    std::fs::create_dir_all(&dir)?;
    let files = repl.list_files()?;
    std::fs::write(dir.join("files.txt"), format!("{files:?}\n\n{identity}"))?;
    for name in ["boot.py", "main.py"] {
        if files.iter().any(|f| f == name) {
            let data = repl.read_file(name)?;
            std::fs::write(dir.join(name), &data)?;
            println!("         backed up {name} ({} bytes)", data.len());
        }
    }
    if !dir.join("main.py").exists() {
        bail!("backup of main.py failed — refusing to continue");
    }

    println!("  [3/4] writing suzu identity + faceplate …");
    let adopted = chrono::Utc::now().date_naive();
    let suzu_json = format!(
        r#"{{"proto":"suzu/1","companion":"firefly","device_id":"{device_id}","family":"esp8266-oled","variant":"oled-v2","faceplate":"portrait-numerals","adopted":"{adopted}"}}"#
    );
    repl.write_file("suzu.json", suzu_json.as_bytes())?;
    repl.write_file("main.py", FACEPLATE_MAIN.as_bytes())?;

    println!("  [4/4] soft reboot, verifying handshake …");
    repl.soft_reboot()?;
    thread::sleep(Duration::from_secs(1));
    match crate::probe::probe(port)? {
        crate::probe::Outcome::Suzu { json, .. } => {
            let ok = json.get("proto").and_then(|v| v.as_str()) == Some("suzu/1");
            let kept = json.get("device_id").and_then(|v| v.as_str()) == Some(device_id.as_str());
            if !(ok && kept) {
                bail!("device answered, but identity did not verify (proto/device_id mismatch)");
            }
            Ok(format!(
                "migrated ✓  proto suzu/1 · device_id preserved ({device_id})"
            ))
        }
        _ => bail!("post-flash probe did not answer as suzu"),
    }
}

/// Restore the pre-suzu firmware from the backup, and remove the suzu
/// identity — the un-migration.
pub fn restore(port: &str) -> Result<String> {
    let identity = identify(port)?;
    let device_id = identity
        .get("device_id")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow::anyhow!("device has no device_id"))?
        .to_string();
    let dir = backup_dir(&device_id);
    let main_backup = std::fs::read(dir.join("main.py"))
        .map_err(|_| anyhow::anyhow!("no backup for {device_id} — refusing to touch the device"))?;

    println!("  restoring pre-suzu main.py from backup …");
    let mut repl = Repl::open(port)?;
    repl.write_file("main.py", &main_backup)?;
    if let Err(e) = repl.remove_file("suzu.json") {
        println!("  note: suzu.json removal said: {e}");
    }
    repl.soft_reboot()?;
    thread::sleep(Duration::from_secs(1));
    match crate::probe::probe(port)? {
        crate::probe::Outcome::Suzu { json, .. } => {
            if json.get("proto").and_then(|v| v.as_str()) == Some("suzu/1") {
                bail!("still answering as suzu — restore did not take");
            }
            Ok("restored ✓ — pre-suzu firmware is back in charge".into())
        }
        _ => Ok("restored ✓ — device answers with its pre-suzu identity".into()),
    }
}

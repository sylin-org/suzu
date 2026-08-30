//! The maintenance sagas — soft and factory reset, step by step,
//! journaled as house events (ADR-0003).
//!
//! A saga owns its port exclusively (the session closed before it
//! began) and follows the class `procedure.yaml`'s declared steps.
//! Two laws bind every path:
//! - **Backup precedes every write** — the individual's identity is
//!   stashed before anything is erased, and restored before the saga
//!   ends.
//! - **The tool that bricks is the tool that un-bricks** — the
//!   runtime artifacts are vendored in `firmware/artifacts/`, so a
//!   factory reset works offline at 2 a.m.; a missing artifact fails
//!   the saga *before* any erase begins.

use crate::catalog::Catalog;
use anyhow::{anyhow, bail, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Sender;

use super::events::HouseEvent;

const ARTIFACTS: &str = "firmware/artifacts";
const RP2040_FIRMWARE: &str = "firmware/suzu-d/rp2040-matrix";

/// The saga's spine: run the kind the keeper asked for, step by step,
/// every step landing on the house bus as it happens.
pub fn run(
    port: &str,
    class: Option<&str>,
    kind: &str,
    _catalog: &Catalog,
    events: &Sender<HouseEvent>,
    device_id: &str,
) -> Result<()> {
    let outcome = match (class, kind) {
        (Some("waveshare-rp2040-matrix"), "install" | "adopt") => {
            // A board with no CircuitPython yet gets the full install:
            // BOOTSEL, the runtime, then the face. One with the drive
            // mounted just needs its files refreshed.
            if circuitpy_drives().is_empty() {
                rp2040_install_fresh(events, device_id)
            } else {
                rp2040_soft(events, device_id)
            }
        }
        (Some("waveshare-rp2040-matrix"), "soft") => rp2040_soft(events, device_id),
        (Some("waveshare-rp2040-matrix"), "factory") => rp2040_factory(events, device_id),
        (Some(c), kind) if c.contains("esp8266") => match kind {
            "install" | "soft" => esp8266_soft(port, events, device_id),
            "adopt" => esp8266_adopt(port, events, device_id),
            "factory" => esp8266_factory(port, events, device_id),
            _ => bail!("no saga for kind {kind:?}"),
        },
        (Some(c), _) => bail!("class {c} declares no maintenance procedure yet"),
        (None, _) => bail!("no class manifest — no maintenance procedure"),
    };
    match &outcome {
        Ok(()) => step(events, device_id, "admission-gate", true,
            "session respawns — only a passing admission test reopens the stream".into()),
        Err(e) => step(events, device_id, "failed", false, format!("{e:#}")),
    }
    outcome
}

fn step(events: &Sender<HouseEvent>, device_id: &str, name: &str, ok: bool, detail: String) {
    let _ = events.send(HouseEvent::MaintenanceStep {
        device_id: device_id.to_string(),
        step: name.to_string(),
        ok,
        detail,
    });
}

fn marco(events: &Sender<HouseEvent>, device_id: &str, name: &str, detail: String) {
    step(events, device_id, name, true, detail);
}

fn wait_for<F: Fn() -> bool>(secs: u64, what: &str, pred: F) -> Result<()> {
    let end = Instant::now() + Duration::from_secs(secs);
    while Instant::now() < end {
        if pred() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(400));
    }
    bail!("waited {secs} s for {what} — it never came")
}

/// A CircuitPython drive announces itself with boot_out.txt.
fn find_circuitpy_drive() -> Option<String> {
    for letter in 'A'..='Z' {
        let root = format!("{letter}:\\");
        if Path::new(&format!("{root}boot_out.txt")).exists() {
            return Some(format!("{letter}:"));
        }
    }
    None
}

fn wait_mount(label: &str, secs: u64) -> Result<String> {
    wait_for(secs, &format!("the {label} drive"), || {
        Path::new(&format!("{label}:/")).exists()
    })?;
    Ok(format!("{label}:/"))
}

/// The fresh CIRCUITPY remount takes a moment after a UF2 lands.
fn wait_circuitpy(secs: u64) -> Result<String> {
    wait_for(secs, "CIRCUITPY", || find_circuitpy_drive().is_some())?;
    let drive = find_circuitpy_drive().unwrap();
    // The drive letter is back; give the FAT a beat before writing.
    std::thread::sleep(Duration::from_millis(800));
    Ok(drive)
}

fn copy_uf2(artifact: &str, mount: &str) -> Result<()> {
    let src = Path::new(ARTIFACTS).join(artifact);
    if !src.exists() {
        bail!(
            "artifact {} is missing — factory reset works offline, so it must be vendored first",
            src.display()
        );
    }
    let data = std::fs::read(&src)?;
    let dest = format!("{mount}/{artifact}");
    for attempt in 1..=4 {
        match std::fs::write(&dest, &data) {
            Ok(()) => return Ok(()),
            Err(e) if attempt < 4 => {
                println!("[maintenance] write {artifact} retry {attempt} ({e}) — the remount races the write");
                std::thread::sleep(Duration::from_millis(900));
            }
            Err(e) => bail!("copy {artifact} to {mount}: {e}"),
        }
    }
    Ok(())
}

fn backup_drive_identity(drive: &str, device_id: &str) -> Result<()> {
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    let dest = format!("backups/{}-{stamp}", drive.trim_end_matches(':'));
    std::fs::create_dir_all(&dest)?;
    for name in ["code.py", "suzu.json", "label.txt"] {
        let p = format!("{drive}/{name}");
        if Path::new(&p).exists() {
            std::fs::copy(&p, format!("{dest}/{name}"))
                .map_err(|e| anyhow!("backup {name}: {e}"))?;
        }
    }
    // The roster's device_id is the identity of record; note it beside
    // the backup so a wipe can never orphan an individual.
    std::fs::write(format!("{dest}/identity.txt"), format!("device_id: {device_id}\n"))?;
    Ok(())
}

/// The face files, with the individual's identity restored.
fn write_face_files(drive: &str, device_id: &str) -> Result<()> {
    let code = std::fs::read(format!("{RP2040_FIRMWARE}/code.py"))?;
    let template = std::fs::read_to_string(format!("{RP2040_FIRMWARE}/suzu.json"))
        .unwrap_or_else(|_| "{\"proto\":\"suzu/1\"}".into());
    let mut suzu: serde_json::Value = serde_json::from_str(&template)
        .map_err(|e| anyhow!("suzu.json template: {e}"))?;
    suzu["device_id"] = serde_json::Value::String(device_id.to_string());
    let sj = serde_json::to_vec_pretty(&suzu)?;

    for (name, data) in [("code.py", code), ("suzu.json", sj)] {
        for attempt in 1..=4 {
            match std::fs::write(format!("{drive}/{name}"), &data) {
                Ok(()) => break,
                Err(e) if attempt < 4 => {
                    std::thread::sleep(Duration::from_millis(900));
                    let _ = e;
                }
                Err(e) => bail!("write {name}: {e}"),
            }
        }
        let back = std::fs::read(format!("{drive}/{name}"))?;
        if back != data {
            bail!("read-back verify failed for {name}");
        }
    }
    Ok(())
}

/// CircuitPython with autoreload disabled does not reload on write —
/// the face is told, politely, to start over (Ctrl-C ×2, Ctrl-D).
fn force_reload(port: &str) -> Result<()> {
    let mut p = serialport::new(port, 115_200)
        .timeout(Duration::from_millis(300))
        .open()
        .map_err(|e| anyhow!("{port}: {e}"))?;
    let _ = p.write_data_terminal_ready(true);
    std::thread::sleep(Duration::from_millis(2500));
    let _ = p.write_all(b"\x03\x03");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(400));
    let _ = p.write_all(b"\x04");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(2500));
    drop(p);
    Ok(())
}

fn run_tool(mut command: std::process::Command, what: &str) -> Result<String> {
    let out = command.output().map_err(|e| anyhow!("{what}: {e}"))?;
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    if !out.status.success() {
        bail!("{what} failed: {}", text.lines().last().unwrap_or("no output").trim());
    }
    Ok(text)
}

fn esptool(port: &str, args: &[&str]) -> Result<String> {
    for exe in ["esptool", "esptool.py"] {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--port").arg(port).arg("--chip");
        // The chip name rides with the class procedure; erase/flash
        // are the same verbs everywhere.
        let mut all = args.to_vec();
        let chip = all.remove(0);
        cmd.arg(chip);
        cmd.args(&all);
        match run_tool(cmd, exe) {
            Ok(text) => return Ok(text),
            Err(e) if exe == "esptool.py" => return Err(e),
            Err(_) => continue, // no `esptool` on PATH — try `esptool.py`
        }
    }
    unreachable!()
}

fn push_face_files(port: &str, device_id: &str, fresh: bool) -> Result<String> {
    let mut cmd = std::process::Command::new("python");
    cmd.args([
        "scripts/push_firmware.py",
        port,
        device_id,
    ]);
    if fresh {
        cmd.arg("--fresh");
    }
    run_tool(cmd, "push_firmware.py")
}

// ── the sagas ──────────────────────────────────────────────────────

/// Every mounted CIRCUITPY drive — more than one means the install
/// could land on the wrong board, so the saga refuses and says so.
fn circuitpy_drives() -> Vec<String> {
    let mut out = Vec::new();
    for letter in 'A'..='Z' {
        if Path::new(&format!("{letter}:/boot_out.txt")).exists() {
            out.push(format!("{letter}:"));
        }
    }
    out
}

/// The fresh-board install: BOOTSEL gate, the CircuitPython runtime,
/// then the face files with the individual's minted identity.
fn rp2040_install_fresh(events: &Sender<HouseEvent>, device_id: &str) -> Result<()> {
    marco(events, device_id, "human-gate",
        "hold BOOTSEL and replug - the saga waits up to 10 minutes for RPI-RP2".into());
    let mount = wait_mount("RPI-RP2", 600)?;

    copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
    marco(events, device_id, "runtime", "CircuitPython UF2 landed".into());
    let drive = wait_circuitpy(120)?;
    marco(events, device_id, "drive", format!("{drive} is up, empty"));

    write_face_files(&drive, device_id)?;
    marco(events, device_id, "face-files", "code.py + suzu.json verified by read-back".into());
    Ok(())
}

fn rp2040_soft(events: &Sender<HouseEvent>, device_id: &str) -> Result<()> {
    let drives = circuitpy_drives();
    if drives.len() > 1 {
        bail!("multiple CIRCUITPY drives mounted ({}) - unplug the other boards so the face lands on the right one", drives.join(", "));
    }
    let drive = drives
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive - replug the device, or hold BOOTSEL for the fresh install"))?;
    marco(events, device_id, "drive", format!("{drive} is up"));
    backup_drive_identity(&drive, device_id)?;
    marco(events, device_id, "backup", format!("{drive} identity stashed in backups/"));
    write_face_files(&drive, device_id)?;
    marco(events, device_id, "face-files", "code.py + suzu.json verified by read-back".into());
    force_reload(&rp2040_port()?)?;
    marco(events, device_id, "reload", "the lake starts over".into());
    Ok(())
}

fn rp2040_factory(events: &Sender<HouseEvent>, device_id: &str) -> Result<()> {
    let drive = find_circuitpy_drive()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device first so identity can be backed up"))?;
    backup_drive_identity(&drive, device_id)?;
    marco(events, device_id, "backup", format!("{drive} identity stashed in backups/"));

    marco(events, device_id, "human-gate",
        "hold BOOTSEL and replug — the saga waits up to 5 minutes for RPI-RP2".into());
    let mount = wait_mount("RPI-RP2", 300)?;

    copy_uf2("flash_nuke.uf2", &mount)?;
    marco(events, device_id, "nuke", "flash_nuke.uf2 landed — every flash cell falls to 0xFF".into());
    std::thread::sleep(Duration::from_millis(2500));
    // The nuke reboots into its own bootloader when done: RPI-RP2 returns.
    let mount = wait_mount("RPI-RP2", 60)?;

    copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
    marco(events, device_id, "runtime", "CircuitPython UF2 landed".into());
    let drive = wait_circuitpy(60)?;
    marco(events, device_id, "drive-again", format!("{drive} is back, empty"));

    write_face_files(&drive, device_id)?;
    marco(events, device_id, "face-files", "code.py + suzu.json verified by read-back".into());
    Ok(())
}

/// An ancestor walks in: the full install (the proven fresh push that
/// gives it the suzu face, identity kept), then the exam decides.
fn esp8266_adopt(port: &str, events: &Sender<HouseEvent>, device_id: &str) -> Result<()> {
    marco(events, device_id, "adopt", "the fresh install - suzu face, identity kept".into());
    let out = push_face_files(port, device_id, true)?;
    marco(events, device_id, "adopt-done", last_line(&out));
    Ok(())
}

fn esp8266_soft(port: &str, events: &Sender<HouseEvent>, device_id: &str) -> Result<()> {
    marco(events, device_id, "push", "the proven push (backup-first inside the script)".into());
    let out = push_face_files(port, device_id, false)?;
    marco(events, device_id, "push-done", last_line(&out));
    Ok(())
}

fn esp8266_factory(port: &str, events: &Sender<HouseEvent>, device_id: &str) -> Result<()> {
    marco(events, device_id, "erase", "esptool erase_flash — the ROM bootloader owns the chip".into());
    let out = esptool(port, &["esp8266", "erase_flash"])?;
    marco(events, device_id, "erase-done", last_line(&out));

    let bin = Path::new(ARTIFACTS).join("micropython-esp8266-1mib.bin");
    if !bin.exists() {
        bail!("artifact {} is missing — vendor it before factory", bin.display());
    }
    marco(events, device_id, "runtime", "flashing MicroPython".into());
    let bin_str = bin.to_string_lossy().into_owned();
    let args = [
        "esp8266",
        "write_flash",
        "--flash_size=detect",
        "0",
        bin_str.as_str(),
    ];
    let out = esptool(port, &args)?;
    marco(events, device_id, "runtime-done", last_line(&out));

    let out = push_face_files(port, device_id, true)?;
    marco(events, device_id, "push-done", last_line(&out));
    Ok(())
}

fn rp2040_port() -> Result<String> {
    for e in crate::enumerate() {
        if let Some(usb) = &e.usb {
            if usb.vid == 0x239a || usb.vid == 0x2e8a {
                return Ok(e.name);
            }
        }
    }
    bail!("no RP2040 CDC port — replug the device")
}

fn last_line(text: &str) -> String {
    text.lines()
        .rev()
        .map(|l| l.trim())
        .find(|l| !l.is_empty())
        .unwrap_or("ok")
        .to_string()
}

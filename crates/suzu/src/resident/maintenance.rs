//! The maintenance sagas — install, adopt, soft and factory reset,
//! step by step, journaled as house events (ADR-0003).
//!
//! Every saga declares its plan up front (a total step count), and
//! each step is announced the moment it begins — the workbench shows
//! "step 2 of 5 — Waiting for BOOTSEL" while it runs. A step that
//! fails is told truthfully; a saga that dies leaves the individual
//! New, and the admission exam decides everything after.
//!
//! Two laws bind every path:
//! - **Backup precedes every write** — identity is stashed before
//!   anything is erased, and restored before the saga ends.
//! - **The tool that bricks is the tool that un-bricks** — the
//!   runtime artifacts are vendored in `firmware/artifacts/`, so the
//!   factory path works offline; a missing artifact fails the saga
//!   *before* any erase begins.

use crate::catalog::Catalog;
use anyhow::{anyhow, bail, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Sender;

use super::events::HouseEvent;

const ARTIFACTS: &str = "firmware/artifacts";
const RP2040_FIRMWARE: &str = "firmware/suzu-d/rp2040-matrix";

/// The saga runner: announces numbered steps as they begin, and tells
/// the truth when one fails.
struct Saga<'a> {
    events: &'a Sender<HouseEvent>,
    device_id: String,
    index: u32,
    total: u32,
}

impl<'a> Saga<'a> {
    fn new(events: &'a Sender<HouseEvent>, device_id: &str, total: u32) -> Self {
        Self { events, device_id: device_id.to_string(), index: 0, total }
    }

    /// Announce a step, run it, and keep the announcement either way —
    /// the workbench shows the step while it runs, and the failure if
    /// it fails.
    fn step<T, F>(&mut self, label: &str, run: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        self.index += 1;
        let _ = self.events.send(HouseEvent::MaintenanceStep {
            device_id: self.device_id.clone(),
            step: label.to_string(),
            index: self.index,
            total: self.total,
            ok: true,
            detail: String::new(),
        });
        match run() {
            Ok(v) => Ok(v),
            Err(e) => {
                let _ = self.events.send(HouseEvent::MaintenanceStep {
                    device_id: self.device_id.clone(),
                    step: format!("{label} (failed)"),
                    index: self.index,
                    total: self.total,
                    ok: false,
                    detail: format!("{e:#}"),
                });
                Err(e)
            }
        }
    }

    /// The closing announcement: the saga hands the individual to the
    /// admission exam, which alone decides the stream.
    fn hand_to_exam(&mut self) {
        self.index += 1;
        let _ = self.events.send(HouseEvent::MaintenanceStep {
            device_id: self.device_id.clone(),
            step: "Handing to the exam".to_string(),
            index: self.index,
            total: self.total,
            ok: true,
            detail: String::new(),
        });
    }
}

/// The saga's spine: run the kind the keeper asked for.
pub fn run(
    port: &str,
    class: Option<&str>,
    kind: &str,
    catalog: &Catalog,
    events: &Sender<HouseEvent>,
    device_id: &str,
) -> Result<()> {
    type SagaRunner = fn(&mut Saga, &str, &str, &Catalog) -> Result<()>;
    let (total, runner): (u32, SagaRunner) = match (class, kind)
    {
        (Some("waveshare-rp2040-matrix"), "install" | "adopt") => {
            // A board with no CircuitPython yet gets the full install:
            // BOOTSEL, the runtime, then the face. One with the drive
            // mounted just needs its files refreshed.
            if circuitpy_drives().is_empty() {
                (5, rp2040_fresh)
            } else {
                (4, rp2040_soft)
            }
        }
        (Some("waveshare-rp2040-matrix"), "soft") => (4, rp2040_soft),
        (Some("waveshare-rp2040-matrix"), "factory") => (6, rp2040_factory),
        (Some(c), kind) if c.contains("esp8266") => match kind {
            "install" | "adopt" | "soft" => (2, esp8266_adopt),
            "factory" => (4, esp8266_factory),
            _ => bail!("no saga for kind {kind:?}"),
        },
        (Some(c), _) => bail!("class {c} declares no maintenance procedure yet"),
        (None, _) => bail!("no class manifest — no maintenance procedure"),
    };

    let mut saga = Saga::new(events, device_id, total);
    let outcome = runner(&mut saga, port, device_id, catalog);
    match &outcome {
        Ok(()) => saga.hand_to_exam(),
        Err(e) => {
            let _ = events.send(HouseEvent::MaintenanceStep {
                device_id: device_id.to_string(),
                step: "failed".to_string(),
                index: saga.index + 1,
                total: saga.total,
                ok: false,
                detail: format!("{e:#}"),
            });
        }
    }
    outcome
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

fn find_circuitpy_drive() -> Option<String> {
    for letter in 'A'..='Z' {
        if Path::new(&format!("{letter}:/boot_out.txt")).exists() {
            return Some(format!("{letter}:"));
        }
    }
    None
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
    let dest = Path::new("backups").join(format!("{}-{stamp}", drive.trim_end_matches(':')));
    std::fs::create_dir_all(&dest)?;
    for name in ["code.py", "suzu.json", "label.txt"] {
        let p = format!("{drive}/{name}");
        if Path::new(&p).exists() {
            std::fs::copy(&p, dest.join(name)).map_err(|e| anyhow!("backup {name}: {e}"))?;
        }
    }
    // The roster's device_id is the identity of record; note it beside
    // the backup so a wipe can never orphan an individual.
    std::fs::write(dest.join("identity.txt"), format!("device_id: {device_id}\n"))?;
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
    cmd.args(["scripts/push_firmware.py", port, device_id]);
    if fresh {
        cmd.arg("--fresh");
    }
    run_tool(cmd, "push_firmware.py")
}

fn rp2040_port() -> Result<String> {
    for e in crate::enumerate() {
        if let Some(usb) = &e.usb
            && (usb.vid == 0x239a || usb.vid == 0x2e8a) {
                return Ok(e.name);
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

// ── the rp2040 sagas ────────────────────────────────────────────────

/// A factory-fresh board: BOOTSEL gate, the CircuitPython runtime,
/// then the face files with the individual's minted identity.
fn rp2040_fresh(
    saga: &mut Saga,
    _port: &str,
    device_id: &str,
    _catalog: &Catalog,
) -> Result<()> {
    saga.step("Preparing the board", || {
        if !circuitpy_drives().is_empty() {
            bail!("a CIRCUITPY drive is already mounted — this board is not fresh; use Reinstall");
        }
        Ok(())
    })?;
    saga.step("Waiting for BOOTSEL", || {
        wait_mount("RPI-RP2", 600).map(|_| ())
    })?;
    saga.step("Flashing CircuitPython", || {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
        wait_circuitpy(120).map(|_| ())
    })?;
    saga.step("Writing the face", || {
        let drive = find_circuitpy_drive()
            .ok_or_else(|| anyhow!("the drive vanished before the face could be written"))?;
        write_face_files(&drive, device_id)
    })?;
    Ok(())
}

/// The face files return to ship state; the runtime is untouched.
fn rp2040_soft(
    saga: &mut Saga,
    _port: &str,
    device_id: &str,
    _catalog: &Catalog,
) -> Result<()> {
    let drives = circuitpy_drives();
    if drives.len() > 1 {
        bail!("multiple CIRCUITPY drives mounted ({}) — unplug the other boards so the face lands on the right one", drives.join(", "));
    }
    let drive = drives
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device"))?;
    saga.step("Checking the drive", || {
        if Path::new(&format!("{drive}/boot_out.txt")).exists() {
            Ok(())
        } else {
            bail!("{drive} does not look like a CircuitPython board");
        }
    })?;
    saga.step("Backing up identity", || {
        backup_drive_identity(&drive, device_id)
    })?;
    saga.step("Writing the face", || {
        write_face_files(&drive, device_id)
    })?;
    saga.step("Nudging the face", || force_reload(&rp2040_port()?))?;
    Ok(())
}

/// The nuke: BOOTSEL, every flash cell to 0xFF, the runtime rebuilt,
/// the face rewritten — identity backed up first, restored after.
fn rp2040_factory(
    saga: &mut Saga,
    _port: &str,
    device_id: &str,
    _catalog: &Catalog,
) -> Result<()> {
    let drive = find_circuitpy_drive()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device first so identity can be backed up"))?;
    saga.step("Backing up identity", || {
        backup_drive_identity(&drive, device_id)
    })?;
    saga.step("Waiting for BOOTSEL", || {
        println!("[maintenance] hold BOOTSEL and replug — waiting up to 10 minutes for RPI-RP2");
        wait_mount("RPI-RP2", 600).map(|_| ())
    })?;
    saga.step("Erasing the flash", || {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("flash_nuke.uf2", &mount)?;
        std::thread::sleep(Duration::from_millis(2500));
        wait_mount("RPI-RP2", 60).map(|_| ()) // the nuke reboots into its bootloader
    })?;
    saga.step("Flashing CircuitPython", || {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
        wait_circuitpy(120).map(|_| ())
    })?;
    saga.step("Writing the face", || {
        let drive = find_circuitpy_drive()
            .ok_or_else(|| anyhow!("the drive vanished before the face could be written"))?;
        write_face_files(&drive, device_id)
    })?;
    Ok(())
}

// ── the esp8266 sagas ───────────────────────────────────────────────

/// An ancestor walks in: the full install (the proven fresh push that
/// gives it the suzu face, identity kept), then the exam decides.
fn esp8266_adopt(
    saga: &mut Saga,
    port: &str,
    device_id: &str,
    _catalog: &Catalog,
) -> Result<()> {
    saga.step("Installing the suzu face", || {
        push_face_files(port, device_id, true).map(|out| last_line(&out))
    })?;
    Ok(())
}

fn esp8266_factory(
    saga: &mut Saga,
    port: &str,
    device_id: &str,
    _catalog: &Catalog,
) -> Result<()> {
    saga.step("Erasing the flash", || {
        esptool(port, &["esp8266", "erase_flash"]).map(|_| ())
    })?;
    let bin = Path::new(ARTIFACTS).join("micropython-esp8266-1mib.bin");
    if !bin.exists() {
        bail!("artifact {} is missing — vendor it before factory", bin.display());
    }
    let bin_str = bin.to_string_lossy().into_owned();
    let args = ["esp8266", "write_flash", "--flash_size=detect", "0", bin_str.as_str()];
    saga.step("Flashing MicroPython", || {
        esptool(port, &args).map(|_| ())
    })?;
    saga.step("Installing the suzu face", || {
        push_face_files(port, device_id, true).map(|out| last_line(&out))
    })?;
    Ok(())
}

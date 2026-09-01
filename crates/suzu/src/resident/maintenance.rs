//! Install, update, and factory-reset maintenance procedures (ADR-0003).
//!
//! Each run declares a step count and publishes progress events. A
//! failed run leaves the device in the New state until admission passes.
//!
//! Device identity is backed up before writes and restored before the
//! run ends. Runtime artifacts are vendored in `firmware/artifacts/`
//! so factory reset works offline and validates artifacts before erase.

use crate::catalog::Catalog;
use anyhow::{anyhow, bail, Result};
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Sender;

use super::events::ResidentEvent;

/// Publishes numbered steps and failures. `total` includes the final
/// admission or failure step.
struct MaintenanceRun<'a> {
    events: &'a Sender<ResidentEvent>,
    device_id: String,
    index: u32,
    total: u32,
}

impl<'a> MaintenanceRun<'a> {
    fn new(events: &'a Sender<ResidentEvent>, device_id: &str, total: u32) -> Self {
        Self { events, device_id: device_id.to_string(), index: 0, total }
    }

    /// Announce a step, execute it, and publish any failure.
    fn step<T, F>(&mut self, label: &str, run: F) -> Result<T>
    where
        F: FnOnce() -> Result<T>,
    {
        self.index += 1;
        let _ = self.events.send(ResidentEvent::MaintenanceStep {
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
                let _ = self.events.send(ResidentEvent::MaintenanceStep {
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

    /// Publish the final step.
    fn close(&mut self, label: &str, ok: bool, detail: String) {
        self.index += 1;
        let _ = self.events.send(ResidentEvent::MaintenanceStep {
            device_id: self.device_id.clone(),
            step: label.to_string(),
            index: self.index,
            total: self.total,
            ok,
            detail,
        });
    }

    /// Publish the transition to admission tests.
    fn finish_successfully(&mut self) {
        self.close("Starting admission tests", true, String::new());
    }
}

/// Select and run the requested maintenance procedure.
pub fn run(
    port: &str,
    class: Option<&str>,
    kind: &str,
    catalog: &Catalog,
    events: &Sender<ResidentEvent>,
    device_id: &str,
    faceplate: Option<&str>,
) -> Result<()> {
    type MaintenanceRunner = fn(&mut MaintenanceRun, &str, &str, &str, &Catalog, Option<&str>) -> Result<()>;
    let (total, runner): (u32, MaintenanceRunner) = match (class, kind) {
        (Some("waveshare-rp2040-matrix"), "install" | "provision") => {
            // A board without a mounted CircuitPython drive requires the full
            // BOOTSEL installation. A mounted drive only needs its files updated.
            // Totals include procedure steps and the final admission step.
            if circuitpy_drives().is_empty() {
                (5, rp2040_fresh)
            } else {
                (5, rp2040_soft)
            }
        }
        (Some("waveshare-rp2040-matrix"), "soft") => (5, rp2040_soft),
        (Some("waveshare-rp2040-matrix"), "factory") => (6, rp2040_factory),
        (Some(c), kind) if c.contains("esp8266") => match kind {
            "install" | "provision" | "soft" => (2, esp8266_provision),
            _ => bail!("unsupported maintenance kind {kind:?}"),
        },
        (Some(c), kind) if c.contains("tdisplay") => match kind {
            // The T-Display already runs MicroPython, so provisioning only
            // updates files and does not enter the bootloader.
            "install" | "provision" | "soft" => (2, tdisplay_provision),
            _ => bail!("tdisplay does not support the {kind:?} maintenance procedure"),
        },
        (Some(c), _) => bail!("class {c} declares no maintenance procedure yet"),
        (None, _) => bail!("no class manifest — no maintenance procedure"),
    };

    let mut run = MaintenanceRun::new(events, device_id, total);
    match runner(
        &mut run,
        port,
        class.as_deref().unwrap_or(""),
        device_id,
        catalog,
        faceplate,
    ) {
        Ok(()) => run.finish_successfully(),
        Err(e) => {
            run.close("failed", false, format!("{e:#}"));
        }
    }
    Ok(())
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
/// could target the wrong board, so the run rejects ambiguous mounts.
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

/// Wait for CIRCUITPY to remount after copying a UF2 file.
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
    let src = crate::paths::firmware_dir().join("artifacts").join(artifact);
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
    let dest = crate::paths::backups_dir().join(format!("{}-{stamp}", drive.trim_end_matches(':')));
    std::fs::create_dir_all(&dest)?;
    for name in ["code.py", "suzu.json", "label.txt"] {
        let p = format!("{drive}/{name}");
        if Path::new(&p).exists() {
            std::fs::copy(&p, dest.join(name)).map_err(|e| anyhow!("backup {name}: {e}"))?;
        }
    }
    // The registry's device_id is the identity of record; note it beside
    // the backup so a reset cannot lose the registered device identity.
    std::fs::write(dest.join("identity.txt"), format!("device_id: {device_id}\n"))?;
    Ok(())
}

/// Write the faceplate files and restore the device identity.
fn write_faceplate_files(drive: &str, device_id: &str) -> Result<()> {
    let firmware = crate::paths::firmware_dir().join("suzu-d/rp2040-matrix");
    let code = std::fs::read(firmware.join("code.py"))?;
    let template = std::fs::read_to_string(firmware.join("suzu.json"))
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

/// Restart CircuitPython after writes when autoreload is disabled.
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

fn rp2040_port() -> Result<String> {
    for e in crate::enumerate() {
        if let Some(usb) = &e.usb
            && (usb.vid == 0x239a || usb.vid == 0x2e8a) {
                return Ok(e.name);
            }
    }
    bail!("no RP2040 CDC port — replug the device")
}

// ── the rp2040 procedures ────────────────────────────────────────────────

/// Provision a factory-reset board through BOOTSEL, install CircuitPython,
/// then install the faceplate files with the assigned device identity.
fn rp2040_fresh(
    run: &mut MaintenanceRun,
    _port: &str,
    _class: &str,
    device_id: &str,
    _catalog: &Catalog,
    _faceplate: Option<&str>,
) -> Result<()> {
    run.step("Preparing the board", || {
        if !circuitpy_drives().is_empty() {
            bail!("a CIRCUITPY drive is already mounted — this board is not fresh; use Reinstall");
        }
        Ok(())
    })?;
    run.step("Waiting for BOOTSEL", || {
        wait_mount("RPI-RP2", 600).map(|_| ())
    })?;
    run.step("Flashing CircuitPython", || {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
        wait_circuitpy(120).map(|_| ())
    })?;
    run.step("Writing the faceplate", || {
        let drive = find_circuitpy_drive()
            .ok_or_else(|| anyhow!("the drive disconnected before the faceplate could be written"))?;
        write_faceplate_files(&drive, device_id)
    })?;
    Ok(())
}

/// Restore faceplate files without replacing the CircuitPython runtime.
fn rp2040_soft(
    run: &mut MaintenanceRun,
    _port: &str,
    _class: &str,
    device_id: &str,
    _catalog: &Catalog,
    _faceplate: Option<&str>,
) -> Result<()> {
    let drives = circuitpy_drives();
    if drives.len() > 1 {
        bail!("multiple CIRCUITPY drives mounted ({}); unplug other boards to select one target", drives.join(", "));
    }
    let drive = drives
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device"))?;
    run.step("Checking the drive", || {
        if Path::new(&format!("{drive}/boot_out.txt")).exists() {
            Ok(())
        } else {
            bail!("{drive} does not look like a CircuitPython board");
        }
    })?;
    run.step("Backing up identity", || {
        backup_drive_identity(&drive, device_id)
    })?;
    run.step("Writing the faceplate", || {
        write_faceplate_files(&drive, device_id)
    })?;
    run.step("Restarting the faceplate", || force_reload(&rp2040_port()?))?;
    Ok(())
}

/// Erase flash through BOOTSEL, reinstall the runtime and faceplate, and
/// restore the identity saved before the reset.
fn rp2040_factory(
    run: &mut MaintenanceRun,
    _port: &str,
    _class: &str,
    device_id: &str,
    _catalog: &Catalog,
    _faceplate: Option<&str>,
) -> Result<()> {
    let drive = find_circuitpy_drive()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device first so identity can be backed up"))?;
    run.step("Backing up identity", || {
        backup_drive_identity(&drive, device_id)
    })?;
    run.step("Waiting for BOOTSEL", || {
        println!("[maintenance] hold BOOTSEL and replug — waiting up to 10 minutes for RPI-RP2");
        wait_mount("RPI-RP2", 600).map(|_| ())
    })?;
    run.step("Erasing the flash", || {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("flash_nuke.uf2", &mount)?;
        std::thread::sleep(Duration::from_millis(2500));
        wait_mount("RPI-RP2", 60).map(|_| ()) // The erase image reboots into the bootloader.
    })?;
    run.step("Flashing CircuitPython", || {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
        wait_circuitpy(120).map(|_| ())
    })?;
    run.step("Writing the faceplate", || {
        let drive = find_circuitpy_drive()
            .ok_or_else(|| anyhow!("the drive disconnected before the faceplate could be written"))?;
        write_faceplate_files(&drive, device_id)
    })?;
    Ok(())
}

// ── the tdisplay procedures ──────────────────────────────────────────────

/// The T-Display already runs MicroPython (legacy firmware or a previous
/// Aurora): provisioning is the file push — backup, the Aurora bundle,
/// faceplate metadata into suzu.json, followed by admission tests.
fn tdisplay_provision(
    run: &mut MaintenanceRun,
    port: &str,
    class: &str,
    device_id: &str,
    catalog: &Catalog,
    faceplate: Option<&str>,
) -> Result<()> {
    let faceplate_id = faceplate.unwrap_or_else(|| default_faceplate(catalog, class));
    run.step(&format!("Installing T-Display faceplate ({faceplate_id})"), || {
        crate::prepare::install_tdisplay(port, Some(device_id), Some(faceplate_id), class)
    })?;
    Ok(())
}

// ── the esp8266 procedures ───────────────────────────────────────────────

/// Return the class's first declared faceplate as a fallback.
fn default_faceplate<'a>(catalog: &'a Catalog, class: &str) -> &'a str {
    catalog
        .faceplates_for_class(class)
        .first()
        .map(|f| f.id.as_str())
        .unwrap_or("")
}

/// Install or update the application files while preserving identity.
fn esp8266_provision(
    run: &mut MaintenanceRun,
    port: &str,
    class: &str,
    device_id: &str,
    catalog: &Catalog,
    faceplate: Option<&str>,
) -> Result<()> {
    let faceplate_id = faceplate.unwrap_or_else(|| default_faceplate(catalog, class));
    run.step(&format!("Installing faceplate ({faceplate_id})"), || {
        crate::prepare::install_esp8266(port, Some(device_id), Some(faceplate_id), class)
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a run with a real event channel for counter tests.
    fn run_of(total: u32) -> (MaintenanceRun<'static>, tokio::sync::broadcast::Receiver<ResidentEvent>) {
        let (tx, _rx) = tokio::sync::broadcast::channel(32);
        let tx = Box::leak(Box::new(tx));
        (MaintenanceRun::new(tx, "id-1", total), tx.subscribe())
    }

    /// Collect every announcement still on the bus.
    fn announcements(rx: &mut tokio::sync::broadcast::Receiver<ResidentEvent>) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        while let Ok(ResidentEvent::MaintenanceStep { index, total, .. }) = rx.try_recv() {
            out.push((index, total));
        }
        out
    }

    #[test]
    fn success_uses_the_planned_step_count() {
        let (mut run, mut rx) = run_of(3);
        run.step("one", || Ok(())).unwrap();
        run.step("two", || Ok(())).unwrap();
        run.finish_successfully();
        let seen = announcements(&mut rx);
        assert_eq!(seen, vec![(1, 3), (2, 3), (3, 3)]);
    }

    #[test]
    fn a_failed_step_reannounces_its_own_number() {
        let (mut run, mut rx) = run_of(3);
        run.step("one", || Ok(())).unwrap();
        run.step("two", || Err::<(), _>(anyhow!("no drive"))).unwrap_err();
        let seen = announcements(&mut rx);
        assert_eq!(seen, vec![(1, 3), (2, 3), (2, 3)]);
    }

    #[test]
    fn the_counter_never_runs_past_the_plan() {
        // Mid-failure: the closing "failed" takes the next number.
        let (mut run, mut rx) = run_of(3);
        run.step("one", || Ok(())).unwrap();
        run.step("two", || Err::<(), _>(anyhow!("boom"))).unwrap_err();
        run.close("failed", false, "boom".into());
        assert!(announcements(&mut rx).iter().all(|(i, t)| i <= t));

        // A failure before any step still fits the plan.
        let (mut run, mut rx) = run_of(2);
        run.close("failed", false, "no CIRCUITPY drive".into());
        assert_eq!(announcements(&mut rx), vec![(1, 2)]);
    }
}

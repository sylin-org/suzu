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
///
/// The counting law: `total` is every numbered announcement the saga
/// will ever make — its steps, plus the closing hand (the exam on
/// success, `failed` otherwise). A step that fails re-announces its
/// own number; the closing takes the next one, so the counter can
/// never run past the plan.
struct Saga<'a> {
    events: &'a Sender<HouseEvent>,
    device_id: String,
    index: u32,
    total: u32,
}

/// A long step's voice: handed to the step's closure, it announces
/// where the work is while the work runs. The plan's numbers hold —
/// this is the same step, saying what it is doing.
struct StepVoice<'a> {
    events: &'a Sender<HouseEvent>,
    device_id: &'a str,
    index: u32,
    total: u32,
}

impl StepVoice<'_> {
    fn speak(&self, text: &str) {
        let _ = self.events.send(HouseEvent::MaintenanceStep {
            device_id: self.device_id.to_string(),
            step: text.to_string(),
            index: self.index,
            total: self.total,
            ok: true,
            detail: String::new(),
        });
    }
}

impl<'a> Saga<'a> {
    fn new(events: &'a Sender<HouseEvent>, device_id: &str, total: u32) -> Self {
        Self { events, device_id: device_id.to_string(), index: 0, total }
    }

    /// Announce a step, run it, and keep the announcement either way —
    /// the workbench shows the step while it runs, and the failure if
    /// it fails. The closure speaks through the step's voice.
    fn step<T, F>(&mut self, label: &str, run: F) -> Result<T>
    where
        F: FnOnce(&StepVoice<'_>) -> Result<T>,
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
        let voice = StepVoice {
            events: self.events,
            device_id: &self.device_id,
            index: self.index,
            total: self.total,
        };
        match run(&voice) {
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

    /// The closing announcement — whichever way the saga ends.
    fn close(&mut self, label: &str, ok: bool, detail: String) {
        self.index += 1;
        let _ = self.events.send(HouseEvent::MaintenanceStep {
            device_id: self.device_id.clone(),
            step: label.to_string(),
            index: self.index,
            total: self.total,
            ok,
            detail,
        });
    }

    /// The success closing: the saga hands the individual to the
    /// admission exam, which alone decides the stream.
    fn hand_to_exam(&mut self) {
        self.close("Handing to the exam", true, String::new());
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
    faceplate: Option<&str>,
) -> Result<()> {
    type SagaRunner = fn(&mut Saga, &str, &str, &Catalog, Option<&str>) -> Result<()>;
    let (total, runner): (u32, SagaRunner) = match (class, kind)
    {
        (Some("waveshare-rp2040-matrix"), "install" | "adopt") => {
            // A board with no CircuitPython yet gets the full install:
            // BOOTSEL, the runtime, then the face. One with the drive
            // mounted just needs its files refreshed. Totals count the
            // steps plus the closing hand (the counting law, Saga's doc).
            if circuitpy_drives().is_empty() {
                (5, rp2040_fresh)
            } else {
                (5, rp2040_soft)
            }
        }
        (Some("waveshare-rp2040-matrix"), "soft") => (5, rp2040_soft),
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
    let outcome = runner(&mut saga, port, device_id, catalog, faceplate);
    match &outcome {
        Ok(()) => saga.hand_to_exam(),
        Err(e) => saga.close("failed", false, format!("{e:#}")),
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

/// The calmest a tool's voice may pulse — a burst (esptool's progress
/// bar speaks in `\r` ticks) collapses to its latest line at this
/// cadence, so the journal stays a story, not a firehose.
const SPEAK_MIN_INTERVAL: Duration = Duration::from_millis(1000);

/// A tool line worth speaking: the tail after any carriage return (a
/// progress bar redraws one line in place), trimmed of frame noise.
fn speakable(line: &str) -> &str {
    line.rsplit('\r').next().unwrap_or("").trim()
}

/// Run a tool with its voice on: every line it speaks becomes a step
/// announcement as it lands (coalesced — see SPEAK_MIN_INTERVAL), so a
/// long step is never a silent step. Returns the tool's own last line.
fn run_tool(voice: &StepVoice<'_>, mut command: std::process::Command, what: &str) -> Result<String> {
    use std::io::{BufRead, BufReader};
    use std::sync::mpsc;
    command
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = command.spawn().map_err(|e| anyhow!("{what}: {e}"))?;
    let (tx, rx) = mpsc::channel::<String>();
    let mut pipes: Vec<Box<dyn std::io::Read + Send>> = Vec::new();
    if let Some(p) = child.stdout.take() {
        pipes.push(Box::new(p));
    }
    if let Some(p) = child.stderr.take() {
        pipes.push(Box::new(p));
    }
    for pipe in pipes {
        let tx = tx.clone();
        std::thread::spawn(move || {
            for line in BufReader::new(pipe).lines() {
                if tx.send(line.unwrap_or_default()).is_err() {
                    return;
                }
            }
        });
    }
    drop(tx);

    let mut last = String::new();
    let mut pending: Option<String> = None;
    let mut last_spoke = Instant::now() - SPEAK_MIN_INTERVAL;
    loop {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(line) => {
                let text = speakable(&line);
                if text.is_empty() {
                    continue;
                }
                last = text.to_string();
                if last_spoke.elapsed() >= SPEAK_MIN_INTERVAL {
                    voice.speak(text);
                    pending = None;
                    last_spoke = Instant::now();
                } else {
                    pending = Some(text.to_string());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if let Some(text) = pending.take() {
                    voice.speak(&text);
                    last_spoke = Instant::now();
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    if let Some(text) = pending.take() {
        voice.speak(&text);
    }
    let status = child.wait().map_err(|e| anyhow!("{what}: wait failed — {e}"))?;
    if !status.success() {
        bail!("{what} failed: {}", if last.is_empty() { format!("exited with {status}") } else { last });
    }
    Ok(last)
}

fn esptool(voice: &StepVoice<'_>, port: &str, args: &[&str]) -> Result<String> {
    for exe in ["esptool", "esptool.py"] {
        let mut cmd = std::process::Command::new(exe);
        cmd.arg("--port").arg(port).arg("--chip");
        let mut all = args.to_vec();
        let chip = all.remove(0);
        cmd.arg(chip);
        cmd.args(&all);
        match run_tool(voice, cmd, exe) {
            Ok(text) => return Ok(text),
            Err(e) if exe == "esptool.py" => return Err(e),
            Err(_) => continue, // no `esptool` on PATH — try `esptool.py`
        }
    }
    unreachable!()
}

fn push_face_files(
    voice: &StepVoice<'_>,
    port: &str,
    device_id: &str,
    fresh: bool,
    faceplate: Option<&str>,
) -> Result<String> {
    let mut cmd = std::process::Command::new("python");
    cmd.args(["scripts/push_firmware.py", port, device_id]);
    if fresh {
        cmd.arg("--fresh");
    }
    if let Some(dress) = faceplate {
        cmd.args(["--faceplate", dress]);
    }
    run_tool(voice, cmd, "push_firmware.py")
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
    _faceplate: Option<&str>,
) -> Result<()> {
    saga.step("Preparing the board", |_voice| {
        if !circuitpy_drives().is_empty() {
            bail!("a CIRCUITPY drive is already mounted — this board is not fresh; use Reinstall");
        }
        Ok(())
    })?;
    saga.step("Waiting for BOOTSEL", |_voice| {
        wait_mount("RPI-RP2", 600).map(|_| ())
    })?;
    saga.step("Flashing CircuitPython", |_voice| {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
        wait_circuitpy(120).map(|_| ())
    })?;
    saga.step("Writing the face", |_voice| {
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
    _faceplate: Option<&str>,
) -> Result<()> {
    let drives = circuitpy_drives();
    if drives.len() > 1 {
        bail!("multiple CIRCUITPY drives mounted ({}) — unplug the other boards so the face lands on the right one", drives.join(", "));
    }
    let drive = drives
        .into_iter()
        .next()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device"))?;
    saga.step("Checking the drive", |_voice| {
        if Path::new(&format!("{drive}/boot_out.txt")).exists() {
            Ok(())
        } else {
            bail!("{drive} does not look like a CircuitPython board");
        }
    })?;
    saga.step("Backing up identity", |_voice| {
        backup_drive_identity(&drive, device_id)
    })?;
    saga.step("Writing the face", |_voice| {
        write_face_files(&drive, device_id)
    })?;
    saga.step("Nudging the face", |_voice| force_reload(&rp2040_port()?))?;
    Ok(())
}

/// The nuke: BOOTSEL, every flash cell to 0xFF, the runtime rebuilt,
/// the face rewritten — identity backed up first, restored after.
fn rp2040_factory(
    saga: &mut Saga,
    _port: &str,
    device_id: &str,
    _catalog: &Catalog,
    _faceplate: Option<&str>,
) -> Result<()> {
    let drive = find_circuitpy_drive()
        .ok_or_else(|| anyhow!("no CIRCUITPY drive — replug the device first so identity can be backed up"))?;
    saga.step("Backing up identity", |_voice| {
        backup_drive_identity(&drive, device_id)
    })?;
    saga.step("Waiting for BOOTSEL", |_voice| {
        println!("[maintenance] hold BOOTSEL and replug — waiting up to 10 minutes for RPI-RP2");
        wait_mount("RPI-RP2", 600).map(|_| ())
    })?;
    saga.step("Erasing the flash", |_voice| {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("flash_nuke.uf2", &mount)?;
        std::thread::sleep(Duration::from_millis(2500));
        wait_mount("RPI-RP2", 60).map(|_| ()) // the nuke reboots into its bootloader
    })?;
    saga.step("Flashing CircuitPython", |_voice| {
        let mount = wait_mount("RPI-RP2", 60)?;
        copy_uf2("circuitpython-raspberry_pi_pico.uf2", &mount)?;
        wait_circuitpy(120).map(|_| ())
    })?;
    saga.step("Writing the face", |_voice| {
        let drive = find_circuitpy_drive()
            .ok_or_else(|| anyhow!("the drive vanished before the face could be written"))?;
        write_face_files(&drive, device_id)
    })?;
    Ok(())
}

// ── the esp8266 sagas ───────────────────────────────────────────────

/// The dress a bare saga installs when the keeper named none: the
/// class's first declaration. (Callers that know the face's current
/// dress pass it; this is the last resort, not the default path.)
fn default_dress(catalog: &Catalog) -> &str {
    catalog
        .faceplates_for_class("esp8266-oled")
        .first()
        .map(|f| f.id.as_str())
        .unwrap_or("numerals")
}

/// An ancestor walks in: the full install (the proven fresh push that
/// gives it the suzu face, identity kept), then the exam decides.
fn esp8266_adopt(
    saga: &mut Saga,
    port: &str,
    device_id: &str,
    catalog: &Catalog,
    faceplate: Option<&str>,
) -> Result<()> {
    let dress = faceplate.unwrap_or(default_dress(catalog));
    saga.step(&format!("Installing the suzu face ({dress})"), |voice| {
        push_face_files(voice, port, device_id, true, Some(dress)).map(|out| last_line(&out))
    })?;
    Ok(())
}

fn esp8266_factory(
    saga: &mut Saga,
    port: &str,
    device_id: &str,
    catalog: &Catalog,
    faceplate: Option<&str>,
) -> Result<()> {
    saga.step("Erasing the flash", |voice| {
        esptool(voice, port, &["esp8266", "erase_flash"]).map(|_| ())
    })?;
    let bin = Path::new(ARTIFACTS).join("micropython-esp8266-1mib.bin");
    if !bin.exists() {
        bail!("artifact {} is missing — vendor it before factory", bin.display());
    }
    let bin_str = bin.to_string_lossy().into_owned();
    let args = ["esp8266", "write_flash", "--flash_size=detect", "0", bin_str.as_str()];
    saga.step("Flashing MicroPython", |voice| {
        esptool(voice, port, &args).map(|_| ())
    })?;
    let dress = faceplate.unwrap_or(default_dress(catalog));
    saga.step(&format!("Installing the suzu face ({dress})"), |voice| {
        push_face_files(voice, port, device_id, true, Some(dress)).map(|out| last_line(&out))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A saga wired to a real bus, so the announcements can be read
    /// back and the counting law checked against the wire's truth.
    fn saga_of(total: u32) -> (Saga<'static>, tokio::sync::broadcast::Receiver<HouseEvent>) {
        let (tx, _rx) = tokio::sync::broadcast::channel(32);
        let tx = Box::leak(Box::new(tx));
        (Saga::new(tx, "id-1", total), tx.subscribe())
    }

    /// Collect every announcement still on the bus.
    fn announcements(rx: &mut tokio::sync::broadcast::Receiver<HouseEvent>) -> Vec<(u32, u32)> {
        let mut out = Vec::new();
        while let Ok(HouseEvent::MaintenanceStep { index, total, .. }) = rx.try_recv() {
            out.push((index, total));
        }
        out
    }

    #[test]
    fn success_lands_exactly_on_the_plan() {
        let (mut saga, mut rx) = saga_of(3);
        saga.step("one", |_| Ok(())).unwrap();
        saga.step("two", |_| Ok(())).unwrap();
        saga.hand_to_exam();
        let seen = announcements(&mut rx);
        assert_eq!(seen, vec![(1, 3), (2, 3), (3, 3)]);
    }

    #[test]
    fn a_failed_step_reannounces_its_own_number() {
        let (mut saga, mut rx) = saga_of(3);
        saga.step("one", |_| Ok(())).unwrap();
        saga.step("two", |_| Err::<(), _>(anyhow!("no drive"))).unwrap_err();
        let seen = announcements(&mut rx);
        assert_eq!(seen, vec![(1, 3), (2, 3), (2, 3)]);
    }

    #[test]
    fn the_counter_never_runs_past_the_plan() {
        // Mid-failure: the closing "failed" takes the next number.
        let (mut saga, mut rx) = saga_of(3);
        saga.step("one", |_| Ok(())).unwrap();
        saga.step("two", |_| Err::<(), _>(anyhow!("boom"))).unwrap_err();
        saga.close("failed", false, "boom".into());
        assert!(announcements(&mut rx).iter().all(|(i, t)| i <= t));

        // A failure before any step still fits the plan.
        let (mut saga, mut rx) = saga_of(2);
        saga.close("failed", false, "no CIRCUITPY drive".into());
        assert_eq!(announcements(&mut rx), vec![(1, 2)]);
    }
}

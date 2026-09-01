//! `suzu prepare` — the adoption front door.
//!
//! Lists every plugged candidate (serial faces and CircuitPython
//! drives), shows its honest state (suzu / unknown / blank),
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
}

/// The faceplates declared in the repo, keyed by class id.
fn faceplates_for(class: &str) -> Vec<FaceplateDecl> {
    let mut out = Vec::new();
    // A class owns its dresses: hardware/classes/<class>/faceplates/*/
    let root = crate::paths::hardware_dir().join("classes");
    let Ok(entries) = std::fs::read_dir(&root) else {
        return out;
    };
    for dir in entries.flatten() {
        let fp_root = dir.path().join("faceplates");
        let Ok(faces) = std::fs::read_dir(&fp_root) else {
            continue;
        };
        for face in faces.flatten() {
            let decl = face.path().join("faceplate.yaml");
            let Ok(text) = std::fs::read_to_string(&decl) else {
                continue;
            };
            if let Ok(d) = serde_yaml::from_str::<FaceplateDecl>(&text)
                && d.class == class {
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
    for slot in &mut b[6..16] {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        *slot = (seed >> 33) as u8;
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

fn install_rp2040(drive: &str, _class: &str) -> anyhow::Result<()> {
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
    let src = crate::paths::firmware_dir().join("suzu-d/rp2040-matrix");
    // Rule zero: backup what the drive holds before writing.
    let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
    // a drive letter brings a colon: invalid in a Windows path component
    let safe_drive = drive.trim_end_matches(':');
    let dest = crate::paths::backups_dir().join(format!("{safe_drive}-{stamp}"));
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
    if existing.exists()
        && let Ok(old) = serde_json::from_str::<serde_json::Value>(
            &std::fs::read_to_string(existing).unwrap_or_default(),
        )
            && let Some(id) = old.get("device_id").and_then(|v| v.as_str()) {
                suzu["device_id"] = serde_json::Value::String(id.to_string());
                println!("  identity preserved: {id}");
            }
    if suzu.get("device_id").map(|v| v == "assigned at adoption (preserved from earlier provisioning when present)").unwrap_or(false) || suzu.get("device_id").is_none() {
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

fn install_esp8266(
    port: &str,
    device_id: Option<&str>,
    faceplate: Option<&str>,
    class: &str,
) -> anyhow::Result<()> {
    // The proven installer, now native: backup-first, chunked writes,
    // per-file verify, soft reboot — the raw-REPL engine the house
    // speaks itself, no interpreter needed on the host.
    let (dress_dir, dress_name, dress_mount, dress_version) =
        resolve_dress(class, faceplate.unwrap_or("numerals"))?;
    println!("  faceplate bundle: {dress_name} v{dress_version}");

    let mut repl = crate::repl::Repl::open(port)?;
    let files = repl.list_files()?;
    println!("device files: {files:?}");
    if files.is_empty() {
        // An unreadable filesystem is a diagnosis, not a blank check.
        // (The esp8266 arrives here only after erase_flash + write_flash
        // through its own recovery procedure — `--fresh` in spirit.)
        println!("  fresh filesystem — writing the first dress");
    }
    repl.backup_files(&files, port)?;

    let (family, variant) = class_signature(class)?;
    let suzu = serde_json::json!({
        "proto": "suzu/1",
        "companion": "firefly",
        "family": family,
        "variant": variant,
        "faceplate": dress_name,
        "adopted": today(),
        "dress_version": dress_version,
    });
    let suzu = {
        let mut s = suzu;
        if let Some(m) = &dress_mount {
            s["mount"] = serde_json::Value::String(m.clone());
        }
        if let Some(id) = device_id.filter(|id| !id.is_empty()) {
            println!("identity preserved: {id}");
            s["device_id"] = serde_json::Value::String(id.to_string());
        }
        s
    };

    // The class's suzu-d firmware ships with the tool; the dress
    // carries its own bootstrap, bytecode, and art. A missing file
    // fails here, before any write — a dress that cannot be read is
    // not a dress to push.
    let fw = crate::paths::firmware_dir().join("suzu-d/esp8266-oled-v2");
    let mut payload: Vec<(String, Vec<u8>)> = Vec::new();
    for name in ["boot.py", "firefly_oled_v2.py", "icons.py", "profont_10.py"] {
        payload.push((
            name.to_string(),
            std::fs::read(fw.join(name))
                .map_err(|e| anyhow::anyhow!("read firmware {name}: {e}"))?,
        ));
    }
    payload.push(("suzu.json".into(), serde_json::to_vec(&suzu)?));
    for name in ["main.py", "face.mpy"] {
        payload.push((
            name.to_string(),
            std::fs::read(dress_dir.join(name))
                .map_err(|e| anyhow::anyhow!("read dress {name}: {e}"))?,
        ));
    }
    for art in bundle_bins(&dress_dir)? {
        payload.push((art.clone(), std::fs::read(dress_dir.join(&art))?));
    }
    for stale in ["main.mpy", "face.py"] {
        if files.iter().any(|f| f == stale) {
            println!("  removing stale {stale} ...");
            repl.remove_file(stale)?;
        }
    }
    println!("pushing {} files to {port} ...", payload.len());
    for (name, data) in &payload {
        repl.write_file(name, data)?;
    }
    repl.soft_reboot()?;
    println!("rebooted into suzu — run `suzu scan` to verify the handshake");
    Ok(())
}

/// The T-Display's own path: the C display driver stays frozen on the
/// board; adoption replaces the application files (suzu.json, the
/// bootstrap, the face's bytecode) and preserves the deed.
fn install_tdisplay(
    port: &str,
    device_id: Option<&str>,
    faceplate: Option<&str>,
    class: &str,
) -> anyhow::Result<()> {
    let (dress_dir, dress_name, dress_mount, dress_version) =
        resolve_dress(class, faceplate.unwrap_or("aurora"))?;
    println!("  faceplate bundle: {dress_name} v{dress_version}");

    let mut repl = crate::repl::Repl::open(port)?;
    let files = repl.list_files()?;
    println!("device files: {files:?}");
    if files.is_empty() {
        bail!(
            "filesystem listing came back empty — refusing to write \
             (recovery first: erase_flash + write_flash, never on a guess)"
        );
    }
    repl.backup_files(&files, port)?;

    // Never wipe a deed by silence: keep what the device carries. The
    // file may be spaced (our own writes) or compact (the face's
    // ujson) — serde handles both, never a format guess.
    let existing = repl.read_file("suzu.json").ok();
    let existing: Option<serde_json::Value> = existing
        .as_deref()
        .and_then(|b| std::str::from_utf8(b).ok())
        .and_then(|s| serde_json::from_str(s).ok());
    if existing.is_some() && device_id.is_none() && {
        let id = existing.as_ref().and_then(|v| v.get("device_id"));
        id.map(|v| v.as_str().unwrap_or_default().is_empty()).unwrap_or(true)
    } {
        bail!(
            "suzu.json exists but gave no device_id — refusing to write \
             an idless dress over a known device (pass the id explicitly)"
        );
    }
    let device_id = device_id
        .map(|s| s.to_string())
        .or_else(|| {
            existing
                .as_ref()
                .and_then(|v| v.get("device_id"))
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
        });
    if let Some(id) = &device_id {
        println!("identity preserved: {id}");
    }

    let (family, variant) = class_signature(class)?;
    let mut suzu = serde_json::json!({
        "proto": "suzu/1",
        "companion": "firefly",
        "family": family,
        "variant": variant,
        "faceplate": dress_name,
        "mount": dress_mount,
        "dress_version": dress_version,
        "adopted": today(),
    });
    if let Some(id) = &device_id {
        suzu["device_id"] = serde_json::Value::String(id.clone());
    }

    let mut payload: Vec<(String, Vec<u8>)> =
        vec![("suzu.json".into(), serde_json::to_vec(&suzu)?)];
    for name in ["main.py", "face.mpy"] {
        payload.push((
            name.to_string(),
            std::fs::read(dress_dir.join(name))
                .map_err(|e| anyhow::anyhow!("read dress {name}: {e}"))?,
        ));
    }
    // A leftover source from an older push would shadow the bytecode.
    if files.iter().any(|f| f == "face.py") {
        println!("  removing stale face.py ...");
        repl.remove_file("face.py")?;
    }
    println!("pushing {} files to {port} ...", payload.len());
    for (name, data) in &payload {
        repl.write_file(name, data)?;
    }
    repl.soft_reboot()?;
    println!("rebooted — the face should answer its HELLO on the bus");
    Ok(())
}

/// A dress id -> (bundle directory, faceplate name, mount side, this
/// hang's version), resolved from the class's own faceplate
/// manifests. Variant-type faceplates declare their hangs in the
/// manifest; single-type faceplates bundle at their own root.
fn resolve_dress(
    class: &str,
    faceplate: &str,
) -> anyhow::Result<(std::path::PathBuf, String, Option<String>, String)> {
    #[derive(serde::Deserialize)]
    struct Manifest {
        name: String,
        #[serde(default)]
        version: Option<String>,
        #[serde(default)]
        mount: Option<String>,
        #[serde(default)]
        variants: Option<Vec<Variant>>,
    }
    #[derive(serde::Deserialize)]
    struct Variant {
        id: String,
        mount: String,
        #[serde(default)]
        version: Option<String>,
    }
    let root = crate::paths::hardware_dir()
        .join("classes")
        .join(class)
        .join("faceplates");
    let mut undeclared = String::new();
    for entry in std::fs::read_dir(&root)?.flatten() {
        let mf = entry.path().join("faceplate.yaml");
        let Ok(text) = std::fs::read_to_string(&mf) else {
            continue;
        };
        let Ok(m) = serde_yaml::from_str::<Manifest>(&text) else {
            continue;
        };
        for v in m.variants.iter().flatten() {
            if v.id == faceplate {
                let side = v
                    .mount
                    .strip_prefix("usb-")
                    .unwrap_or(&v.mount)
                    .to_string();
                let version = v
                    .version
                    .clone()
                    .or_else(|| m.version.clone())
                    .unwrap_or_else(|| "0.0.0".into());
                return Ok((entry.path().join(format!("{side}-mount")), m.name, Some(side), version));
            }
        }
        if m.name == faceplate && m.variants.is_none() {
            let mount = m
                .mount
                .as_deref()
                .map(|s| s.strip_prefix("usb-").unwrap_or(s).to_string());
            let version = m.version.unwrap_or_else(|| "0.0.0".into());
            return Ok((entry.path(), m.name, mount, version));
        }
        undeclared.push_str(&m.name);
        undeclared.push(' ');
    }
    bail!("faceplate {faceplate:?} is not declared for {class} — declared: {undeclared}")
}

/// The class's wire signature (family + variant), read from its own
/// signature.yaml — the same words the descriptor must say.
fn class_signature(class: &str) -> anyhow::Result<(String, String)> {
    #[derive(serde::Deserialize)]
    struct Sig {
        family: String,
        variant: String,
    }
    let path = crate::paths::hardware_dir()
        .join("classes")
        .join(class)
        .join("signature.yaml");
    let sig: Sig = serde_yaml::from_str(&std::fs::read_to_string(path)?)?;
    Ok((sig.family, sig.variant))
}

fn bundle_bins(dir: &std::path::Path) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.ends_with(".bin") {
            out.push(name);
        }
    }
    out.sort();
    Ok(out)
}

fn today() -> String {
    chrono::Utc::now().format("%Y-%m-%d").to_string()
}

pub fn run(catalog: &Catalog, args: &[String]) -> anyhow::Result<()> {
    // The scripted path: `suzu prepare PORT [FACEPLATE] [--id ID]` —
    // how a fresh deployment adopts a face with no prompts and no
    // interpreter on the host.
    if let Some(port) = args.iter().find(|a| !a.starts_with('-')).cloned() {
        return run_direct(catalog, &port, args);
    }
    run_interactive(catalog)
}

fn run_direct(catalog: &Catalog, port: &str, args: &[String]) -> anyhow::Result<()> {
    let faceplate = args.iter().skip(1).find(|a| !a.starts_with('-')).cloned();
    let device_id = args
        .iter()
        .position(|a| a == "--id")
        .and_then(|i| args.get(i + 1).cloned());
    let Some(e) = crate::enumerate().into_iter().find(|e| e.name == port) else {
        bail!("{port} is not plugged in");
    };
    let Some(usb) = &e.usb else {
        bail!("{port} has no USB identity to classify");
    };
    let Some(class) = catalog.class_id_for(usb.vid, usb.pid) else {
        bail!("{port} is not a declared class");
    };
    let device_id = device_id.or_else(|| {
        probe::probe_transcript(port)
            .identity
            .as_ref()
            .and_then(|j| j.get("device_id"))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    match class.as_str() {
        "esp8266-oled" => {
            install_esp8266(port, device_id.as_deref(), faceplate.as_deref(), &class)?
        }
        "tdisplay-esp32-ch9102" => {
            install_tdisplay(port, device_id.as_deref(), faceplate.as_deref(), &class)?
        }
        other => bail!("no direct install path for {other:?} yet"),
    }
    verify_and_report(port);
    Ok(())
}

fn verify_and_report(port: &str) {
    println!("verifying ...");
    let t = probe::probe_transcript(port);
    match &t.identity {
        Some(json) => println!(
            "  verified: proto {} · version {} · device_id {}",
            json.get("proto").and_then(|v| v.as_str()).unwrap_or("?"),
            json.get("version").and_then(|v| v.as_str()).unwrap_or("?"),
            json.get("device_id").and_then(|v| v.as_str()).unwrap_or("?")
        ),
        None => println!("  no identity yet — the face may need a moment; run `suzu scan`"),
    }
}

fn run_interactive(catalog: &Catalog) -> anyhow::Result<()> {
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
        Some("esp8266-oled") => {
            install_esp8266(
                &cand.name,
                cand.device_id.as_deref(),
                faceplate.as_deref(),
                "esp8266-oled",
            )?;
        }
        Some("tdisplay-esp32-ch9102") => {
            install_tdisplay(
                &cand.name,
                cand.device_id.as_deref(),
                faceplate.as_deref(),
                "tdisplay-esp32-ch9102",
            )?;
        }
        other => bail!("no install path for {other:?} yet — the class needs a procedure"),
    }

    verify_and_report(&cand.name);
    println!("prepare complete.");
    Ok(())
}

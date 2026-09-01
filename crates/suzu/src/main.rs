//! Suzu device discovery, maintenance, and diagnostics CLI.
//!
//! Subcommands:
//!   (none)     watch USB serial ports; identify on hotplug; service
//!   scan       one-shot identification of every connected port
//!   list       list and manage the Resident's compatible devices
//!   detective  full diagnostic output per USB device
//!   serve      run the Resident service
//!
//! Maintenance procedures are defined in the Resident maintenance module.

mod bootloader;
mod catalog;
mod control;
mod gif;
mod resident_cli;
mod mpush;
mod paths;
mod prepare;
mod repl;
mod probe;
mod resident;
mod servicing;
mod shot;

use catalog::Catalog;
use probe::{Outcome, Transcript};
use serialport::SerialPortType;
use std::collections::BTreeMap;
use std::io::{self, Write};
use std::sync::mpsc::{self, Sender};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// What the identification sequence concluded about a port.
#[derive(Clone)]
struct Verdict {
    /// One line, styled like: `NEW     ESP8266 + OLED display (CH340)`.
    short: String,
    /// Extra lines worth showing under the verdict (empty is fine).
    detail: String,
}

impl Verdict {
    fn new(short: impl Into<String>, detail: impl Into<String>) -> Self {
        Self {
            short: short.into(),
            detail: detail.into(),
        }
    }
}

// ── USB enumeration ────────────────────────────────────────────────

pub struct UsbInfo {
    pub vid: u16,
    pub pid: u16,
    pub product: Option<String>,
    pub manufacturer: Option<String>,
    pub serial: Option<String>,
}

pub struct PortEntry {
    pub name: String,
    pub usb: Option<UsbInfo>,
}

fn enumerate() -> Vec<PortEntry> {
    serialport::available_ports()
        .unwrap_or_default()
        .into_iter()
        .map(|p| {
            let usb = match &p.port_type {
                SerialPortType::UsbPort(u) => Some(UsbInfo {
                    vid: u.vid,
                    pid: u.pid,
                    product: u.product.clone(),
                    manufacturer: u.manufacturer.clone(),
                    serial: u.serial_number.clone(),
                }),
                _ => None,
            };
            PortEntry { name: p.port_name, usb }
        })
        .collect()
}

// ── verdicts ───────────────────────────────────────────────────────

/// Legacy CSV identity → variant token: `OK,firefly-oled,esp8266,…`
/// yields `oled`, which is what a class signature matches on.
fn legacy_variant_token(line: &str) -> Option<String> {
    let rest = line.trim().strip_prefix("OK,")?;
    let rest = rest.strip_prefix("firefly-")?;
    let token = rest.split(',').next()?.trim();
    (!token.is_empty()).then(|| token.to_string())
}

/// Verdict from an already-run identification transcript.
fn verdict_from(catalog: &Catalog, vid: u16, pid: u16, t: &Transcript) -> Verdict {
    if let Some(json) = &t.identity {
        let family = json.get("family").and_then(|v| v.as_str()).unwrap_or("suzu");
        let variant = json.get("variant").and_then(|v| v.as_str()).unwrap_or("");
        let version = json.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        let proto = json.get("proto").and_then(|v| v.as_str()).unwrap_or("");
        let class = catalog.class_by_signature(family, variant);
        let head = match class {
            Some(c) => format!("{} · {} v{version}", c.id, family),
            None => format!("{family} v{version}"),
        };
        let tag = if proto.is_empty() {
            "unknown descriptor".to_string()
        } else {
            format!("[{proto}]")
        };
        let mut detail = format!("{tag}\n      {json}");
        if t.hello {
            detail = format!("spoke first (HELLO)\n      {detail}");
        }
        return Verdict::new(head, detail);
    }
    if let Some(line) = &t.legacy_line {
        let token = legacy_variant_token(line);
        let class = token
            .as_deref()
            .and_then(|tok| catalog.class_by_signature("firefly", tok));
        let head = match class {
            Some(c) => format!("{} · firefly (not installed)", c.id),
            None => "firefly (not installed)".to_string(),
        };
        return Verdict::new(
            head,
            format!("{line}\n      → migrate with the `install` action"),
        );
    }
    if let Some(c) = catalog.class_by_vidpid(vid, pid) {
        return Verdict::new(
            format!("NEW     {} ({}/{})", c.id, c.family, c.variant),
            "no identity response — fresh firmware",
        );
    }
    if let Some(h) = catalog::seed_hint(vid, pid) {
        return Verdict::new(format!("NEW     {h}"), "no identity response — fresh firmware");
    }
    Verdict::new(
        "unknown serial device",
        format!("not in catalog ({vid:04x}:{pid:04x})"),
    )
}

/// The identification sequence + catalog join. Tool-side verdict.
fn identify(catalog: &Catalog, port_name: &str, vid: u16, pid: u16) -> Verdict {
    let t = probe::probe_transcript(port_name);
    if let Some(e) = &t.error {
        return Verdict::new(
            "unreachable",
            format!("{e}\n      (stale, busy, or non-responding port)"),
        );
    }
    verdict_from(catalog, vid, pid, &t)
}

// ── scan (one-shot) ────────────────────────────────────────────────

fn scan_once(catalog: &Catalog) {
    let ports = enumerate();
    if ports.is_empty() {
        println!("no serial ports found — plug the device in");
        return;
    }
    for e in &ports {
        let Some(u) = &e.usb else {
            println!("  {:<14} non-USB serial port", e.name);
            continue;
        };
        print!("  {:<14} ", e.name);
        let _ = io::stdout().flush();
        let v = identify(catalog, &e.name, u.vid, u.pid);
        println!("{}", v.short);
        for line in v.detail.lines() {
            println!("      {line}");
        }
    }
}

// ── detective (full fact dump) ─────────────────────────────────────

fn draft_signature(
    family: &str,
    variant: &str,
    version: Option<&str>,
    vid: u16,
    pid: u16,
) -> String {
    let id = if family == "TODO" {
        format!("unknown-{:04x}{:04x}", vid, pid)
    } else {
        format!("{family}-{variant}")
    };
    let mut s = format!(
        "schema: 1\nid: {id}-class\nfamily: {family}\nvariant: {variant}\n"
    );
    if let Some(v) = version {
        s.push_str(&format!("firmware_line: \"{v}\"\n"));
    }
    s.push_str(&format!("match:\n  vid_pid: [\"{vid:04x}:{pid:04x}\"]\n"));
    s
}

fn detective(catalog: &Catalog) {
    println!("suzu detective — facts on connected USB serial devices\n");
    let mut count = 0;
    for e in &enumerate() {
        let Some(u) = &e.usb else {
            println!("── {} ── non-USB serial port (skipped)", e.name);
            continue;
        };
        count += 1;
        println!("── {} ─────────────────────────────────────", e.name);
        println!("  usb:          {:04x}:{:04x}", u.vid, u.pid);
        println!("  product:      {}", u.product.as_deref().unwrap_or("—"));
        println!(
            "  manufacturer: {}",
            u.manufacturer.as_deref().unwrap_or("—")
        );
        println!("  serial:       {}", u.serial.as_deref().unwrap_or("—"));

        let t = probe::probe_transcript(&e.name);
        let v = verdict_from(catalog, u.vid, u.pid, &t);
        println!("  verdict:      {}", v.short);
        println!(
            "  probe:        HELLO first: {} · identity: {} · after: {}",
            if t.hello { "yes" } else { "no" },
            if t.identity.is_some() {
                "yes"
            } else if t.legacy_line.is_some() {
                "legacy"
            } else {
                "no"
            },
            match t.identity_after_ms {
                Some(ms) => format!("{ms} ms"),
                None => "— (timeout)".to_string(),
            }
        );
        if !t.lines.is_empty() {
            println!("  lines seen:");
            for l in &t.lines {
                println!("    | {l}");
            }
        }
        if let Some(err) = &t.error {
            println!("  error: {err}");
        }

        // Catalog context: signature match vs bridge hint.
        let by_vp = catalog.class_by_vidpid(u.vid, u.pid).map(|c| c.id.clone());
        let by_sig = t.identity.as_ref().and_then(|j| {
            let f = j.get("family").and_then(|v| v.as_str())?;
            let var = j.get("variant").and_then(|v| v.as_str())?;
            catalog.class_by_signature(f, var).map(|c| c.id.clone())
        });
        println!(
            "  catalog:      {}",
            match (&by_sig, &by_vp) {
                (Some(s), Some(v)) if s == v => format!("signature match → {s}"),
                (Some(s), Some(v)) => format!("signature match → {s}; vid/pid hint → {v}"),
                (Some(s), None) => format!("signature match → {s}"),
                (None, Some(v)) => format!("vid/pid hint → {v}"),
                _ => "no match — candidate for a new class".to_string(),
            }
        );

        // Draft: the first file of a new class folder, ready to paste.
        let (family, variant, version) = t
            .identity
            .as_ref()
            .map(|j| {
                (
                    j.get("family")
                        .and_then(|v| v.as_str())
                        .unwrap_or("TODO")
                        .to_string(),
                    j.get("variant")
                        .and_then(|v| v.as_str())
                        .unwrap_or("TODO")
                        .to_string(),
                    j.get("version")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string()),
                )
            })
            .unwrap_or(("TODO".into(), "TODO".into(), None));
        let draft = draft_signature(&family, &variant, version.as_deref(), u.vid, u.pid);
        println!("  draft — hardware/classes/<id>/signature.yaml:");
        for line in draft.lines() {
            println!("    {line}");
        }
        println!();
    }
    if count == 0 {
        println!("no USB serial devices found.");
    }
    println!("detective done — attach the facts to a class proposal or a new hardware/classes/<id>/ folder.");
}

// ── watch mode ─────────────────────────────────────────────────────

fn print_table(devices: &BTreeMap<String, Verdict>) {
    println!("\n── devices ──────────────────────────────────────");
    if devices.is_empty() {
        println!("  (none — plug something in)");
    }
    for (i, (port, v)) in devices.iter().enumerate() {
        println!("  {:>2}. {:<14} {}", i + 1, port, v.short);
        for line in v.detail.lines() {
            println!("      {line}");
        }
    }
    println!("─────────────────────────────────────────────────");
    println!("  type a number to service · q to quit");
    let _ = io::stdout().flush();
}

enum Event {
    Ports(BTreeMap<String, Verdict>),
    Input(String),
}

fn scanner(tx: Sender<Event>, catalog: &Catalog) {
    let mut seen: BTreeMap<String, Verdict> = BTreeMap::new();
    loop {
        let mut changed = false;
        let ports = enumerate();

        seen.retain(|name, _| {
            let present = ports.iter().any(|p| p.name == *name);
            if !present {
                changed = true;
            }
            present
        });

        for e in &ports {
            if !seen.contains_key(&e.name) {
                changed = true;
                let verdict = match &e.usb {
                    Some(u) => identify(catalog, &e.name, u.vid, u.pid),
                    None => Verdict::new(
                        "non-USB serial port",
                        "no USB descriptor — foreign by default",
                    ),
                };
                seen.insert(e.name.clone(), verdict);
            }
        }

        if changed && tx.send(Event::Ports(seen.clone())).is_err() {
            return;
        }
        thread::sleep(Duration::from_millis(1000));
    }
}

fn run_test(port: &str) {
    println!("  probing {port} …");
    let _ = io::stdout().flush();
    match probe::probe(port) {
        Ok(Outcome::Suzu { json, hello }) => {
            println!(
                "  suzu companion{}",
                if hello { " (spoke first)" } else { "" }
            );
            if let Ok(pretty) = serde_json::to_string_pretty(&json) {
                for line in pretty.lines() {
                    println!("    {line}");
                }
            }
        }
        Ok(Outcome::LegacyFirefly { line }) => {
            println!("  unknown firefly identity: {line}");
            println!("  → the device responded; use the install procedure to migrate it.");
        }
        Ok(Outcome::Silent) => println!("  no identity response — fresh or foreign firmware"),
        Err(e) => println!("  probe failed: {e}"),
    }
}

/// Capture one PNG per connected display. The capture uses the serial protocol
/// (J,{"shot":1}); a valid J reply confirms the session without a separate
/// probe, identity check, or reboot. Each port's bytes are decoded per its
/// class manifest's frame format; anything that doesn't answer or has no
/// declared frame produces one result line without modifying the port.
fn screenshot(catalog: &Catalog, filter: Option<&str>) {
    println!("suzu screenshot — one PNG per connected display");
    let ports: Vec<_> = enumerate()
        .into_iter()
        .filter(|e| e.usb.is_some()) // non-USB ports are foreign by default
        .filter(|e| filter.is_none_or(|f| e.name == f))
        .collect();
    if ports.is_empty() {
        println!("  no matching USB serial device; use a data-capable cable");
        return;
    }
    let mut shots = 0;
    for e in &ports {
        print!("  {} … ", e.name);
        let _ = io::stdout().flush();
        let u = e.usb.as_ref().expect("filtered to USB above");
        let Some(class_id) = catalog.class_id_for(u.vid, u.pid) else {
            println!("no shot: {:04x}:{:04x} is not in the catalog — no manifest to decode with", u.vid, u.pid);
            continue;
        };
        let Some(spec) = catalog.frame(&class_id).cloned() else {
            println!("no shot: class {class_id} declares no frame format");
            continue;
        };
        let zones = catalog.display_zones(&class_id);
        match shot::capture(&e.name, spec.size) {
            Ok(frame) => {
                let path = std::path::PathBuf::from(format!("shot-{}.png", e.name));
                match shot::render_png(&path, &spec, &zones, &frame) {
                    Ok(()) => {
                        println!("shot → {} [{class_id}]", path.display());
                        shots += 1;
                    }
                    Err(err) => println!("frame captured, render failed: {err}"),
                }
            }
            Err(err) => println!("no shot: {err}"),
        }
    }
    println!("{shots} png(s) captured without stopping device sessions");
}

fn servicing_menu(port: &str) {
    println!("\n── servicing {port} ──────────────────────────────");
    println!("  1. test          probe & show identity");
    println!("  2. install       flash suzu firmware        (planned)");
    println!("  3. update        upgrade suzu firmware      (planned)");
    println!("  4. factory wipe  erase everything           (planned)");
    println!("  5. back");
    print!("  choose: ");
    let _ = io::stdout().flush();
}

fn watch(catalog: &Arc<Catalog>) {
    println!("suzu — device provisioning and diagnostics");
    println!("watches USB serial ports; new devices are identified automatically.");

    let (tx, rx) = mpsc::channel::<Event>();
    {
        let tx = tx.clone();
        let catalog = Arc::clone(catalog);
        thread::spawn(move || scanner(tx, &catalog));
    }
    {
        let tx = tx.clone();
        thread::spawn(move || {
            let stdin = io::stdin();
            let mut line = String::new();
            loop {
                line.clear();
                match stdin.read_line(&mut line) {
                    Ok(0) | Err(_) => return,
                    Ok(_) => {
                        if tx.send(Event::Input(line.trim().to_string())).is_err() {
                            return;
                        }
                    }
                }
            }
        });
    }

    let mut latest: BTreeMap<String, Verdict> = BTreeMap::new();
    let mut servicing: Option<String> = None;

    for ev in rx {
        match ev {
            Event::Ports(map) => {
                latest = map;
                if servicing.is_none() {
                    print_table(&latest);
                }
            }
            Event::Input(line) => {
                let line = line.trim().to_string();
                match servicing.as_ref() {
                    None => match line.as_str() {
                        "" | "r" => print_table(&latest),
                        "q" | "quit" => break,
                        _ => {
                            if let Ok(n) = line.parse::<usize>() {
                                let names: Vec<String> = latest.keys().cloned().collect();
                                if let Some(port) = names.get(n.saturating_sub(1)) {
                                    servicing = Some(port.clone());
                                    servicing_menu(port);
                                } else {
                                    println!("  no such device — see the table above");
                                }
                            } else {
                                println!("  ? — type a number, or q");
                            }
                        }
                    },
                    Some(port) => {
                        match line.as_str() {
                            "1" => run_test(port),
                            "2" | "3" | "4" => {
                                let verb = ["", "install", "update", "factory wipe"]
                                    [line.parse::<usize>().unwrap_or(0)];
                                println!(
                                    "  {verb}: not implemented in this build.\n  → detection, \
                                     identification and test are available; flashing is handled by the \
                                     procedure engine (docs/hardware-catalog-and-adoption.md §4)."
                                );
                            }
                            "5" | "b" | "back" => {
                                servicing = None;
                                print_table(&latest);
                                continue;
                            }
                            "" => servicing_menu(port),
                            _ => println!("  ? — 1..5"),
                        }
                        servicing_menu(port);
                    }
                }
            }
        }
    }
    println!("stopped");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Arc::new(catalog::Catalog::load());
    if args.get(1).map(String::as_str) != Some("list") {
        println!("catalog: {}", catalog.source);
    }

    match args.get(1).map(|s| s.as_str()) {
        Some("scan") => scan_once(&catalog),
        Some("detective") => detective(&catalog),
        Some("list") => resident_cli::run(&args[2..]).await?,
        Some("serve") => resident::run(catalog).await?,
        Some("screenshot") => screenshot(&catalog, args.get(2).map(|s| s.as_str())),
        Some("prepare") => prepare::run(&catalog, &args[2..])?,
        Some("record") => {
            // Clamp recording length and rate to serial and GIF limits.
            let secs: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(10);
            let fps: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(3);
            let (secs, fps) = (secs.clamp(1, 60), fps.clamp(1, 5));
            let want_port = args.get(4).map(|s| s.as_str());
            if args.len() > 2 {
                println!("recording {secs} s at {fps} fps (limits: ≤60 s, ≤5 fps)");
            }
            // Include every decodable display in enumeration order, or one selected port.
            let mut targets: Vec<shot::CaptureTarget> = Vec::new();
            for e in enumerate().into_iter().filter(|e| e.usb.is_some()) {
                if let Some(w) = want_port
                    && e.name != w {
                        continue;
                    }
                let u = e.usb.as_ref().expect("filtered to USB above");
                let Some(class_id) = catalog.class_id_for(u.vid, u.pid) else {
                    continue;
                };
                let Some(spec) = catalog.frame(&class_id).cloned() else {
                    continue;
                };
                let zones = catalog.display_zones(&class_id);
                targets.push(shot::CaptureTarget {
                    port: e.name.clone(),
                    class: class_id,
                    spec,
                    zones,
                });
            }
            if targets.is_empty() {
                anyhow::bail!("no decodable display; check that the device class manifest declares a frame");
            }
            let (path, n) = shot::record_first(&targets, secs, fps, "record")?;
            println!("{n} frames → {}", path.display());
        }
        Some("say") => {
            // ADR-0006: [port] [signal] [text…]. A port targets one
            // device; without a port the signal is broadcast.
            let text = args[2..].join(" ");
            if text.is_empty() {
                anyhow::bail!("usage: suzu say [port] [signal] <text>  (e.g. suzu say COM24 INFO Hello!)");
            }
            control::send_control(&format!("say {text}")).await?;
        }
        Some("show") => {
            let text = args[2..].join(" ");
            if text.is_empty() {
                anyhow::bail!("usage: suzu show <tag> <text ...>  (e.g. suzu show INFO.disk Disk at 50%)");
            }
            control::send_control(&format!("show {text}")).await?;
        }
        Some("pause") => control::send_control("pause").await?,
        Some("resume") => control::send_control("resume").await?,
        Some("firmware") => {
            let port = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: suzu firmware <port>"))?;
            println!("{}", servicing::migrate(port)?);
        }
        Some("restore") => {
            let port = args.get(2).ok_or_else(|| anyhow::anyhow!("usage: suzu restore <port>"))?;
            println!("{}", servicing::restore(port)?);
        }
        _ => watch(&catalog),
    }
    Ok(())
}

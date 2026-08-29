//! suzu — the adoption, servicing & detective tool.
//!
//! Subcommands:
//!   (none)     watch USB serial ports; identify on hotplug; service
//!   scan       one-shot identification of every connected port
//!   detective  full fact dump per USB device — the harvest instrument
//!   serve      the Resident: watcher · devices · moments · sensor,
//!              talking to each other in the open
//!
//! Servicing today: test. install / update / factory-wipe land with the
//! procedure engine (docs/hardware-catalog-and-adoption.md §4).

mod catalog;
mod control;
mod mpush;
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

/// What the ladder concluded about a port.
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

/// Verdict from an already-run ladder transcript.
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
            "pre-suzu descriptor".to_string()
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
            Some(c) => format!("{} · firefly (pre-suzu firmware)", c.id),
            None => "firefly (pre-suzu firmware)".to_string(),
        };
        return Verdict::new(
            head,
            format!("{line}\n      → migrate with `install` once flashing lands"),
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

/// The identification ladder + catalog join. Tool-side verdict.
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
            println!("  pre-suzu firefly identity: {line}");
            println!("  → still alive. Migration lands with the install procedure.");
        }
        Ok(Outcome::Silent) => println!("  no identity response — fresh or foreign firmware"),
        Err(e) => println!("  probe failed: {e}"),
    }
}

/// One PNG per connected firefly. The capture rides the wire
/// (J,{"shot":1}); only suzu/1 faces answer it — everyone else gets
/// one honest line and untouched ports.
fn screenshot(filter: Option<&str>) {
    println!("suzu screenshot — one png per connected firefly");
    let ports: Vec<_> = enumerate()
        .into_iter()
        .filter(|e| e.usb.is_some()) // non-USB ports are foreign by default
        .filter(|e| filter.map_or(true, |f| e.name == f))
        .collect();
    if ports.is_empty() {
        println!("  no USB serial port matches — plug a firefly in (data cable, not charge-only)");
        return;
    }
    let mut shots = 0;
    for e in &ports {
        println!("  {} …", e.name);
        let _ = io::stdout().flush();
        let t = probe::probe_transcript(&e.name);
        if let Some(err) = &t.error {
            println!("    probe failed: {err}");
            continue;
        }
        let Some(json) = &t.identity else {
            println!("    no shot: no identity response (fresh or foreign firmware)");
            continue;
        };
        let device_id = json
            .get("device_id")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .chars()
            .take(8)
            .collect::<String>();
        // The model, in the class-naming convention: family + variant
        // with the shared tail deduped (esp8266-oled + oled-v2 ->
        // esp8266-oled-v2), filename-safe.
        let family = json.get("family").and_then(|v| v.as_str()).unwrap_or("firefly");
        let variant = json.get("variant").and_then(|v| v.as_str()).unwrap_or("");
        let mut model = match family.rsplit_once('-') {
            Some((_, last)) if variant.starts_with(&format!("{last}-")) => {
                format!("{family}-{}", &variant[last.len() + 1..])
            }
            _ => format!("{family}-{variant}"),
        };
        if variant.is_empty() {
            model = family.to_string();
        }
        let model: String = model
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
            .collect();
        match shot::capture(&e.name) {
            Ok(frame) => {
                let base = format!("shot-{}-{model}-{device_id}", e.name);
                let portrait = std::path::PathBuf::from(format!("{base}.png"));
                let native = std::path::PathBuf::from(format!("{base}-native.png"));
                match shot::render(&frame, &portrait, &native) {
                    Ok(()) => {
                        println!("    shot → {}", portrait.display());
                        shots += 1;
                    }
                    Err(err) => println!("    frame lifted, render failed: {err}"),
                }
            }
            Err(err) => println!("    no shot: {err}"),
        }
    }
    println!("{shots} png(s) — faces were rebooted to clean up; `suzu serve` dresses them again");
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
    println!("suzu — adoption, servicing & detective");
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
                                     identification and test are live; flashing lands with the \
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
    println!("bye — the garden keeps breathing.");
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    let catalog = Arc::new(catalog::Catalog::load());
    println!("catalog: {}", catalog.source);

    match args.get(1).map(|s| s.as_str()) {
        Some("scan") => scan_once(&catalog),
        Some("detective") => detective(&catalog),
        Some("serve") => resident::run(catalog).await?,
        Some("screenshot") => screenshot(args.get(2).map(|s| s.as_str())),
        Some("pause") => control::chirp("pause").await?,
        Some("resume") => control::chirp("resume").await?,
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

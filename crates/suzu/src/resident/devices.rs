//! The devices domain — the manager of minded devices, and the
//! consumers of published ground.
//!
//! Each live device owns an outbound queue and a session thread that
//! translates the published object into the surface its limitations
//! accept, then pushes device-shaped data to the device itself.

use super::events::{DeviceFacts, HouseEvent};
use super::sensor::MachineReport;
use serde::Serialize;
use serialport::SerialPort;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc};
use std::time::Duration;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DeviceState {
    Accepted,
    #[allow(dead_code)] // used by the servicing engine (unplug mid-pipeline)
    Disposed,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub facts: DeviceFacts,
    pub state: DeviceState,
    pub minded_at: String,
    /// The outbound queue — this device's consumer mailbox.
    #[serde(skip)]
    pub outbound: Option<std_mpsc::Sender<Outbound>>,
}

impl Device {
    pub fn device_id(&self) -> Option<&str> {
        self.facts.device_id.as_deref()
    }
}

/// What a device's consumer mailbox accepts. `Ground` carries the full
/// published object as a cheap `Arc` copy — the device's consumer does
/// the translation.
pub enum Outbound {
    Ground(Arc<MachineReport>),
    /// The pulse lane — fast atoms for faces that declared the extra.
    /// suzu sessions forward them as `A,<axis>,<value>`; ancestor
    /// sessions drop them at this boundary.
    Pulse { axis: String, value: u8 },
    /// A moment for faces: the ring overlay. The signal names the
    /// moment (and its icon, when the face has one).
    Ring { signal: String, words: Vec<String>, urgency: u8 },
    Close,
}

/// Cheap snapshot — copies, safe to hold from any surface.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceRow {
    pub port: String,
    pub class: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub version: Option<String>,
    pub proto: Option<String>,
    pub device_id: Option<String>,
    pub state: DeviceState,
}

pub enum DevicesCmd {
    Mind(DeviceFacts),
    Gone { port: String },
    /// The publisher's outbound pipeline: one call, every live consumer.
    Publish(Arc<MachineReport>),
    Pulse { axis: String, value: u8 },
    Snapshot { reply: mpsc::Sender<Vec<DeviceRow>> },
    /// The control chirp: stop streaming and release the ports (the
    /// faces fall idle into their animations), then re-open and
    /// republish. In-memory only — it dies with the process.
    Pause,
    Resume,
    /// A moment bound for faces: the band shows the label briefly;
    /// the signal names an icon when the face has one.
    Ring { signal: String, label: String, urgency: u8 },
}

pub struct Devices {
    events: Sender<HouseEvent>,
    devices: BTreeMap<String, Device>,
    pulse_announced: bool,
    paused: bool,
}

impl Devices {
    pub fn new(events: Sender<HouseEvent>) -> Self {
        Self {
            events,
            devices: BTreeMap::new(),
            pulse_announced: false,
            paused: false,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<DevicesCmd>) {
        while let Some(cmd) = rx.recv().await {
            match cmd {
                DevicesCmd::Mind(facts) => self.mind(facts),
                DevicesCmd::Gone { port } => self.gone(&port),
                DevicesCmd::Publish(ground) => {
                    if !self.paused {
                        self.publish(&ground)
                    }
                }
                DevicesCmd::Pulse { axis, value } => {
                    if !self.paused {
                        self.pulse(&axis, value)
                    }
                }
                DevicesCmd::Snapshot { reply } => {
                    let _ = reply.send(self.snapshot()).await;
                }
                DevicesCmd::Pause => self.pause_stream(),
                DevicesCmd::Resume => self.resume_stream().await,
                DevicesCmd::Ring { signal, label, urgency } => {
                    if !self.paused {
                        self.ring(&signal, &label, urgency)
                    }
                }
            }
        }
    }

    /// Classes with a known consumer translation. Others are minded but
    /// stay silent until their dialect is codified.
    fn supports_consumer(class: Option<&str>) -> bool {
        class == Some("esp8266-oled-v2-class")
    }

    /// One session thread per live device, owning its port and
    /// translating ground → device-shaped data. A device answering
    /// suzu/1 gets the suzu surface; everything else with a known
    /// translation gets the ancestor vocabulary.
    fn spawn_session(&mut self, facts: &DeviceFacts) {
        let suzu = facts.proto.as_deref() == Some("suzu/1");
        if !suzu && !Self::supports_consumer(facts.class.as_deref()) {
            return; // minded, but no consumer translation yet — silent
        }
        let (tx, rx) = std_mpsc::channel::<Outbound>();
        let close = Arc::new(AtomicBool::new(false));
        let port = facts.port.clone();
        let close2 = Arc::clone(&close);
        let _ = std::thread::Builder::new()
            .name(format!("session:{port}"))
            .spawn(move || session_thread(port, rx, close2, suzu));
        if let Some(device) = self.devices.get_mut(&facts.port) {
            device.outbound = Some(tx);
        }
    }

    /// A ring: every live session tells its face that something
    /// happened. The frame carries the moment's words after the seq.
    fn ring(&mut self, signal: &str, label: &str, urgency: u8) {
        let words: Vec<String> = label.split_whitespace().map(|s| s.to_string()).collect();
        for device in self.devices.values_mut() {
            if let Some(outbound) = &device.outbound {
                let _ = outbound.send(Outbound::Ring {
                    signal: signal.to_string(),
                    words: words.clone(),
                    urgency,
                });
            }
        }
    }

    /// Pause: sessions close, ports release, the ground stops. The
    /// faces fall idle into their animations; the devices stay minded
    /// so `resume` re-opens without replug.
    fn pause_stream(&mut self) {
        if self.paused {
            return;
        }
        self.paused = true;
        for device in self.devices.values_mut() {
            if let Some(outbound) = device.outbound.take() {
                let _ = outbound.send(Outbound::Close);
            }
        }
        println!(
            "[devices] stream paused — {} port(s) released, faces fall idle (`suzu resume` to restart)",
            self.devices.len()
        );
    }

    /// Resume: sessions re-open and the DeviceMinded events send the
    /// publisher republishing its last ground, so faces redress at once.
    async fn resume_stream(&mut self) {
        if !self.paused {
            return;
        }
        self.paused = false;
        let ports: Vec<String> = self.devices.keys().cloned().collect();
        println!("[devices] stream resumed — re-opening {} session(s)", ports.len());
        for port in ports {
            let facts = self.devices[&port].facts.clone();
            self.spawn_session(&facts);
            let _ = self.events.send(HouseEvent::DeviceMinded {
                port,
                device_id: facts.device_id.clone(),
                class: facts.class.clone(),
                state: format!("{:?}", DeviceState::Accepted),
            });
        }
    }

    fn mind(&mut self, facts: DeviceFacts) {
        let state = DeviceState::Accepted;
        match self.devices.get(&facts.port) {
            Some(existing) if existing.device_id() == facts.device_id.as_deref() => {
                let _ = self.events.send(HouseEvent::DeviceHomecoming {
                    port: facts.port.clone(),
                    device_id: facts.device_id.clone().unwrap_or_default(),
                });
            }
            _ => {
                let _ = self.events.send(HouseEvent::DeviceMinded {
                    port: facts.port.clone(),
                    device_id: facts.device_id.clone(),
                    class: facts.class.clone(),
                    state: format!("{state:?}"),
                });
            }
        }

        self.devices.insert(
            facts.port.clone(),
            Device {
                facts: facts.clone(),
                state,
                minded_at: now(),
                outbound: None,
            },
        );
        // A paused house stays silent: the session spawns on resume.
        if !self.paused {
            self.spawn_session(&facts);
        }
    }

    fn gone(&mut self, port: &str) {
        if let Some(mut device) = self.devices.remove(port) {
            // Close the consumer first; the session thread ends and the
            // port is released with it.
            if let Some(outbound) = device.outbound.take() {
                let _ = outbound.send(Outbound::Close);
            }
            let _ = self.events.send(HouseEvent::DeviceReleased {
                port: port.to_string(),
                device_id: device.device_id().map(|s| s.to_string()),
            });
        }
    }

    /// Fan-out: every live consumer takes the full published object as
    /// a cheap copy and translates on its own side. A consumer whose
    /// mailbox is gone (its thread died) is disposed here — the port
    /// is released and the watcher's next cycle can re-mind it.
    fn publish(&mut self, ground: &Arc<MachineReport>) {
        let mut dead: Vec<String> = Vec::new();
        for device in self.devices.values_mut() {
            if let Some(outbound) = &device.outbound {
                if outbound
                    .send(Outbound::Ground(Arc::clone(ground)))
                    .is_err()
                {
                    dead.push(device.facts.port.clone());
                }
            }
        }
        for port in dead {
            println!("[devices] {port}: consumer died — disposing");
            self.gone(&port);
        }
    }

    /// The lane forwards to every live session; ancestors drop it at
    /// the session boundary, suzu faces that declared the extra hear it.
    fn pulse(&mut self, axis: &str, value: u8) {
        let mut consumers = 0;
        let mut dead: Vec<String> = Vec::new();
        for device in self.devices.values_mut() {
            if let Some(outbound) = &device.outbound {
                consumers += 1;
                if outbound
                    .send(Outbound::Pulse {
                        axis: axis.to_string(),
                        value,
                    })
                    .is_err()
                {
                    dead.push(device.facts.port.clone());
                }
            }
        }
        for port in dead {
            println!("[devices] {port}: consumer died — disposing");
            self.gone(&port);
        }
        if !self.pulse_announced {
            self.pulse_announced = true;
            println!(
                "[devices] pulse lane alive: {axis}={value} across {consumers} consumer(s)"
            );
        }
    }

    pub fn snapshot(&self) -> Vec<DeviceRow> {
        self.devices
            .values()
            .map(|d| DeviceRow {
                port: d.facts.port.clone(),
                class: d.facts.class.clone(),
                family: d.facts.family.clone(),
                variant: d.facts.variant.clone(),
                version: d.facts.version.clone(),
                proto: d.facts.proto.clone(),
                device_id: d.facts.device_id.clone(),
                state: d.state.clone(),
            })
            .collect()
    }
}

// ── the consumer — one session thread per device ───────────────────

fn session_thread(
    port: String,
    rx: std_mpsc::Receiver<Outbound>,
    close: Arc<AtomicBool>,
    suzu: bool,
) {
    let mut serial = match open_serial(&port) {
        Ok(p) => p,
        Err(e) => {
            println!("[sessions] {port}: open failed — {e} (device stays idle)");
            return; // rx dropped → the device is simply silent
        }
    };
    println!(
        "[sessions] {port}: consumer translating ({})",
        if suzu { "suzu/1" } else { "ancestor" }
    );

    // A session that panics releases the port with a name on the way
    // out — never a silent thread death holding hardware hostage.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session_loop(&mut serial, &rx, &close, suzu, &port);
    }));
    if outcome.is_err() {
        println!("[sessions] {port}: session panicked — port released");
    }
    println!("[sessions] {port}: released — fireflies when idle");
}

fn session_loop(
    serial: &mut Box<dyn SerialPort>,
    rx: &std_mpsc::Receiver<Outbound>,
    close: &Arc<AtomicBool>,
    suzu: bool,
    port: &str,
) {
    // The ancestor firmware enters its dashboard on first data; a suzu
    // face needs no greeting — its context rides the first ground.
    if !suzu {
        let _ = write_line(serial, "H,thriving");
        let _ = write_line(serial, "G,0,1,0,0");
    }
    let mut named: Option<String> = None;
    let mut seq: u8 = 0;

    loop {
        if close.load(Ordering::Relaxed) {
            break;
        }
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Outbound::Ground(g)) => {
                let frames = if suzu {
                    translate_suzu(&g, &mut named)
                } else {
                    translate(&g)
                };
                for frame in frames {
                    if write_line(serial, &frame).is_err() {
                        println!("[sessions] {port}: write failed — disposing");
                        return;
                    }
                }
            }
            Ok(Outbound::Pulse { axis, value }) => {
                // A suzu face that declared the extra hears the lane;
                // others drop it silently at this boundary.
                if suzu {
                    let frame = format!("A,{axis},{value}");
                    if write_line(serial, &frame).is_err() {
                        println!("[sessions] {port}: write failed — disposing");
                        return;
                    }
                }
            }
            Ok(Outbound::Ring { signal, words, urgency }) => {
                if suzu {
                    seq = seq.wrapping_add(1);
                    let mut frame = format!(
                        "R,{signal},{urgency},0,1,{seq},{}",
                        words.join(",")
                    );
                    let mut x = 0u8;
                    for b in frame.bytes() {
                        x ^= b;
                    }
                    frame = with_checksum(&frame);
                    if write_line(serial, &frame).is_err() {
                        println!("[sessions] {port}: write failed — disposing");
                        return;
                    }
                }
            }
            Ok(Outbound::Close) => break,
            // Keepalive: a suzu face rests after 10 s of silence, the
            // ancestor idles to its fireflies — a frame every 5 s
            // holds either face.
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                let keepalive = if suzu { "K" } else { "R" };
                let _ = write_line(serial, keepalive);
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// The suzu/1 translation: context first (`J`, when the house name is
/// new), then ground.set in the faceplate's declared slot order. A
/// slot the house doesn't measure is 255 — the face draws a dash,
/// never a zero.
fn translate_suzu(g: &MachineReport, named: &mut Option<String>) -> Vec<String> {
    let mut frames = Vec::new();
    if named.as_deref() != Some(g.name.as_str()) {
        frames.push(format!("J,{{\"name\":\"{}\"}}", g.name.replace('"', "'")));
        *named = Some(g.name.clone());
    }
    frames.push(format!(
        "G,report,{},{},{}",
        g.cpu,
        g.mem,
        g.gpu.unwrap_or(255)
    ));
    frames
}

/// The consumer translation: the published object → the surface this
/// device's limitations accept (OLED v2 dashboard vocabulary).
fn translate(g: &MachineReport) -> Vec<String> {
    vec![
        format!("S,{}", g.name),
        format!("H,{}", health_for(g)),
        format!(
            "D,{},{},{},{},0,1,0,{}",
            g.cpu,
            g.mem,
            g.disk,
            fmt_uptime(g.uptime_s),
            0
        ),
    ]
}

fn health_for(g: &MachineReport) -> &'static str {
    let worst = g.cpu.max(g.mem).max(g.disk);
    if worst > 95 {
        "wilting"
    } else if worst > 85 {
        "withering"
    } else {
        "thriving"
    }
}

fn fmt_uptime(secs: u64) -> String {
    if secs >= 86_400 {
        format!("{}d {}h", secs / 86_400, (secs % 86_400) / 3600)
    } else if secs >= 3_600 {
        format!("{}h {}m", secs / 3600, (secs % 3600) / 60)
    } else if secs >= 60 {
        format!("{}m", secs / 60)
    } else {
        "<1m".to_string()
    }
}

fn open_serial(port: &str) -> anyhow::Result<Box<dyn serialport::SerialPort>> {
    let mut p = serialport::new(port, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| anyhow::anyhow!("{port}: {e}"))?;
    // CircuitPython gates its CDC console on DTR: without it the face
    // hears no ground and gardens forever while its neighbors work
    // (proven 2026-08-29 — the matrix idle through a live stream).
    let _ = p.write_data_terminal_ready(true);
    // ESP auto-reset on open — the harvest's 2.5 s boot, plus a settle.
    std::thread::sleep(Duration::from_millis(2500));
    let _ = p.write_all(b"\r\n");
    let _ = p.flush();
    std::thread::sleep(Duration::from_millis(300));
    Ok(p)
}

/// suzu-t session frames carry `*hh` — the xor of everything before.
fn with_checksum(frame: &str) -> String {
    let mut x = 0u8;
    for b in frame.bytes() {
        x ^= b;
    }
    format!("{frame}*{x:02x}")
}

fn write_line(serial: &mut Box<dyn serialport::SerialPort>, line: &str) -> anyhow::Result<()> {
    use std::io::Write;
    serial.write_all(line.as_bytes())?;
    serial.write_all(b"\n")?;
    serial.flush()?;
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

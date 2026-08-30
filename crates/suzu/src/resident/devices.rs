//! The devices domain — the manager of minded devices, and the
//! consumers of published ground.
//!
//! Each live device owns a session thread that owns its port
//! exclusively: stream translation, the J shot, the trail-camera
//! record, and the admission test all ride that one thread, because a
//! serial port answers one master. Whether the stream *flows* is not
//! this domain's decision — the roster grants the subscription
//! (ADR-0003), and this domain merely obeys the gate.

use super::admission;
use super::events::{DeviceFacts, HouseEvent};
use super::roster::Roster;
use super::sensor::MachineReport;
use crate::catalog::{Catalog, FrameSpec};
use serde::Serialize;
use serialport::SerialPort;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::{broadcast::Sender, mpsc};

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DeviceState {
    Accepted,
    #[allow(dead_code)] // used by the servicing engine (unplug mid-pipeline)
    Disposed,
}

/// The trail camera's shared state: what the record job is doing, and
/// the latest frame it lifted — recording subsumes the preview, so the
/// workbench's shot endpoint serves these frames while a record runs.
#[derive(Debug, Default, Clone, Serialize)]
pub struct RecordState {
    /// idle · recording · done
    pub phase: String,
    pub frames: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif_path: Option<String>,
    #[serde(skip)]
    pub latest_png: Option<Vec<u8>>,
}

impl RecordState {
    pub fn is_recording(&self) -> bool {
        self.phase == "recording"
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub facts: DeviceFacts,
    pub state: DeviceState,
    pub minded_at: String,
    /// The session mailbox — this device's consumer.
    #[serde(skip)]
    pub outbound: Option<std_mpsc::Sender<SessionMsg>>,
    /// The roster's gate, mirrored here for the session thread to read
    /// at wire speed. The roster is the truth; this is the echo.
    #[serde(skip)]
    pub streaming: Arc<AtomicBool>,
    /// When the face last heard from the house — the honest aliveness
    /// signal ("spoke 4s ago") beats any checklist.
    #[serde(skip)]
    pub last_fed: Arc<Mutex<Option<Instant>>>,
}

impl Device {
    pub fn device_id(&self) -> Option<&str> {
        self.facts.device_id.as_deref()
    }
}

/// What a device's session mailbox accepts. `Ground` carries the full
/// published object as a cheap `Arc` copy — the session does the
/// translation on its own side of the port.
pub enum SessionMsg {
    Out(Outbound),
    /// One in-band shot, decoded per the class manifest, as PNG bytes.
    Capture { reply: std_mpsc::SyncSender<Vec<u8>> },
    /// The trail camera: exclusive on the session until done.
    Record { secs: u32, fps: u32, state: Arc<Mutex<RecordState>> },
    /// Re-run the admission exam (roster decides what it means).
    Admission,
    Close,
}

pub enum Outbound {
    Ground(Arc<MachineReport>),
    Pulse { axis: String, value: u8 },
    Ring { signal: String, words: Vec<String>, urgency: u8 },
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
    /// The roster's lifecycle verdict for this individual, if known.
    pub lifecycle: Option<String>,
    /// Whether the stream currently flows to this device.
    pub streaming: bool,
    /// Seconds since the face last heard from the house.
    pub last_data_s: Option<u64>,
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
    /// re-publish. In-memory only — it dies with the process.
    Pause,
    Resume,
    /// A moment bound for faces: the band shows the label briefly;
    /// the signal names an icon when the face has one.
    Ring { signal: String, label: String, urgency: u8 },
    /// One in-band shot as PNG bytes, through the owning session.
    Capture { port: String, reply: std_mpsc::SyncSender<Vec<u8>> },
    /// Save one shot into the captures folder; replies with the path.
    CaptureSave { port: String, reply: mpsc::Sender<anyhow::Result<String>> },
    /// The trail camera, on the owning session.
    RecordStart { port: String, secs: u32, fps: u32, reply: mpsc::Sender<anyhow::Result<()>> },
    RecordStatus { port: String, reply: mpsc::Sender<Option<RecordState>> },
    /// Re-run the admission exam through the owning session.
    AdmissionRetry { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    /// The keeper lifted one device off the stream (per-device pause):
    /// the gate closes, the session stays, the face falls to its
    /// garden. Resume re-subscribes without a re-test.
    PauseDevice { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    ResumeDevice { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    /// Hand the individual to a maintenance saga: the session closes,
    /// the port goes to the saga, the stream returns only after the
    /// saga's admission test passes.
    MaintenanceStart { port: String, kind: String, reply: mpsc::Sender<anyhow::Result<()>> },
    /// The saga's own end — it loops back through the door so the
    /// devices domain (which owns the sessions) respawns the face.
    MaintenanceFinished {
        port: String,
        device_id: String,
        ok: bool,
        fresh: Option<DeviceFacts>,
    },
}

pub struct Devices {
    events: Sender<HouseEvent>,
    door: mpsc::Sender<DevicesCmd>,
    roster: Arc<std::sync::RwLock<Roster>>,
    catalog: Arc<Catalog>,
    devices: BTreeMap<String, Device>,
    sessions: BTreeMap<String, SessionHandle>,
    records: BTreeMap<String, Arc<Mutex<RecordState>>>,
    in_maintenance: BTreeMap<String, String>,
    pulse_announced: bool,
    paused: bool,
}

struct SessionHandle {
    close: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for SessionHandle {
    fn drop(&mut self) {
        self.close.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Devices {
    pub fn new(
        events: Sender<HouseEvent>,
        door: mpsc::Sender<DevicesCmd>,
        catalog: Arc<Catalog>,
        roster: Arc<std::sync::RwLock<Roster>>,
    ) -> Self {
        Self {
            events,
            door,
            roster,
            catalog,
            devices: BTreeMap::new(),
            sessions: BTreeMap::new(),
            records: BTreeMap::new(),
            in_maintenance: BTreeMap::new(),
            pulse_announced: false,
            paused: false,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<DevicesCmd>, mut bus: Receiver<HouseEvent>) {
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        DevicesCmd::Mind(facts) => self.mind(facts),
                        DevicesCmd::Gone { port } => self.gone(&port),
                        DevicesCmd::Publish(ground) => {
                            if !self.paused { self.publish(&ground) }
                        }
                        DevicesCmd::Pulse { axis, value } => {
                            if !self.paused { self.pulse(&axis, value) }
                        }
                        DevicesCmd::Snapshot { reply } => {
                            let _ = reply.send(self.snapshot()).await;
                        }
                        DevicesCmd::Pause => self.pause_stream(),
                        DevicesCmd::Resume => self.resume_stream().await,
                        DevicesCmd::Ring { signal, label, urgency } => {
                            if !self.paused { self.ring(&signal, &label, urgency) }
                        }
                        DevicesCmd::Capture { port, reply } => {
                            self.capture(&port, reply);
                        }
                        DevicesCmd::CaptureSave { port, reply } => {
                            let res = self.capture_save(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::RecordStart { port, secs, fps, reply } => {
                            let res = self.record_start(&port, secs, fps);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::RecordStatus { port, reply } => {
                            let _ = reply.send(self.record_status(&port)).await;
                        }
                        DevicesCmd::AdmissionRetry { port, reply } => {
                            let res = self.admission_retry(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::PauseDevice { port, reply } => {
                            let res = self.pause_device(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::ResumeDevice { port, reply } => {
                            let res = self.resume_device(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::MaintenanceStart { port, kind, reply } => {
                            let res = self.maintenance_begin(&port, &kind);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::MaintenanceFinished { port, device_id, ok, fresh } => {
                            self.maintenance_finish(&port, &device_id, ok, fresh).await;
                        }
                    }
                }
                ev = bus.recv() => {
                    match ev {
                        Ok(HouseEvent::StreamAttached { port, .. }) => {
                            if let Some(d) = self.devices.get_mut(&port) {
                                d.streaming.store(true, Ordering::Relaxed);
                            }
                        }
                        Ok(HouseEvent::StreamDetached { port, .. }) => {
                            if let Some(d) = self.devices.get_mut(&port) {
                                d.streaming.store(false, Ordering::Relaxed);
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
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

    fn frame_law_of(&self, facts: &DeviceFacts) -> (Option<FrameSpec>, Vec<(usize, usize, [u8; 3])>) {
        let class_id = facts
            .device_id
            .as_deref()
            .and_then(|_| self.catalog.class_id_for(facts.vid, facts.pid));
        let spec = class_id.as_deref().and_then(|id| self.catalog.frame(id)).cloned();
        let zones = class_id
            .as_deref()
            .map(|id| self.catalog.display_zones(id))
            .unwrap_or_default();
        (spec, zones)
    }

    /// One session thread per live device, owning its port and
    /// translating ground → device-shaped data. The session opens with
    /// the admission exam; the roster's verdict opens the stream gate.
    fn spawn_session(&mut self, facts: &DeviceFacts) {
        let suzu = facts.proto.as_deref() == Some("suzu/1");
        if !suzu && !Self::supports_consumer(facts.class.as_deref()) {
            return; // minded, but no consumer translation yet — silent
        }
        // One master per port: an older session (homecoming without a
        // gone, resume after resume) is closed and joined first.
        self.close_session(&facts.port);
        let (tx, rx) = std_mpsc::channel::<SessionMsg>();
        let (spec, zones) = self.frame_law_of(facts);
        let streaming = Arc::new(AtomicBool::new(false));
        let close = Arc::new(AtomicBool::new(false));
        let port = facts.port.clone();
        let events = self.events.clone();
        let device_id = facts.device_id.clone();
        let class = facts.class.clone();
        let streaming2 = Arc::clone(&streaming);
        let close2 = Arc::clone(&close);
        let join = std::thread::Builder::new()
            .name(format!("session:{port}"))
            .spawn(move || {
                session_thread(
                    port, rx, close2, streaming2, suzu, spec, zones, events, device_id, class,
                )
            })
            .ok();
        self.sessions
            .insert(facts.port.clone(), SessionHandle { close, join });
        if let Some(device) = self.devices.get_mut(&facts.port) {
            device.outbound = Some(tx);
            device.streaming = streaming;
        }
    }

    fn close_session(&mut self, port: &str) {
        if let Some(mut handle) = self.sessions.remove(port) {
            handle.close.store(true, Ordering::Relaxed);
            if let Some(outbound) = self.devices.get_mut(port) {
                if let Some(out) = outbound.outbound.take() {
                    let _ = out.send(SessionMsg::Close);
                }
                outbound.streaming.store(false, Ordering::Relaxed);
            }
            if let Some(join) = handle.join.take() {
                let _ = join.join();
            }
        }
    }

    /// A ring: every live session tells its face that something
    /// happened. The frame carries the moment's words after the seq.
    fn ring(&mut self, signal: &str, label: &str, urgency: u8) {
        let words: Vec<String> = label.split_whitespace().map(|s| s.to_string()).collect();
        for device in self.devices.values_mut() {
            if let Some(outbound) = &device.outbound {
                let _ = outbound.send(SessionMsg::Out(Outbound::Ring {
                    signal: signal.to_string(),
                    words: words.clone(),
                    urgency,
                }));
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
        let ports: Vec<String> = self.devices.keys().cloned().collect();
        for port in &ports {
            self.close_session(port);
            if let Some(device) = self.devices.get_mut(port) {
                device.outbound = None;
            }
        }
        println!(
            "[devices] stream paused — {} port(s) released, faces fall idle (`suzu resume` to restart)",
            self.devices.len()
        );
    }

    /// Resume: sessions re-open (each re-taking its admission exam),
    /// and the publisher republishes its last ground so admitted faces
    /// redress at once.
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

    fn mind(&mut self, mut facts: DeviceFacts) {
        let state = DeviceState::Accepted;
        // Adoption begins at first sight: a recognized class with no
        // identity yet is minted one on the spot. It is written to the
        // device when firmware is installed, and survives everything
        // after (ADR-0003: identity is the name, not the silicon).
        if facts.class.is_some() && facts.device_id.is_none() {
            facts.device_id = Some(crate::prepare::mint_v7());
            println!(
                "[devices] {} minted identity {}",
                facts.port,
                facts.device_id.as_deref().unwrap_or("?")
            );
        }
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
                streaming: Arc::new(AtomicBool::new(false)),
                last_fed: Arc::new(Mutex::new(None)),
            },
        );
        // A paused house stays silent: the session spawns on resume.
        if !self.paused {
            self.spawn_session(&facts);
        }
    }

    fn gone(&mut self, port: &str) {
        if let Some(mut device) = self.devices.remove(port) {
            self.close_session(port);
            device.outbound = None;
            let _ = self.events.send(HouseEvent::DeviceReleased {
                port: port.to_string(),
                device_id: device.device_id().map(|s| s.to_string()),
            });
            // A dead session is not a departure. If the port is still
            // enumerated, the individual is invited back after a
            // settle — the roster remembers them, admission decides
            // again. An unplugged face simply never comes back.
            let facts = device.facts.clone();
            let door = self.door.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let still_present = crate::enumerate().iter().any(|e| e.name == facts.port);
                if still_present {
                    println!("[devices] {} is still on the bench — inviting the individual back", facts.port);
                    let _ = door.send(DevicesCmd::Mind(facts)).await;
                }
            });
        }
    }

    /// The streaming gate: the roster's verdict, checked per fan-out.
    /// A free function of its inputs — the fan-out borrows `devices`
    /// mutably while asking the roster, and the two never meet.
    fn may_stream(roster: &std::sync::RwLock<Roster>, device: &Device) -> bool {
        device.streaming.load(Ordering::Relaxed)
            && device
                .device_id()
                .is_some_and(|id| roster.read().map(|r| r.is_streaming(id)).unwrap_or(false))
    }

    /// Fan-out: every live consumer takes the full published object as
    /// a cheap copy and translates on its own side. A consumer whose
    /// mailbox is gone (its thread died) is disposed here — the port
    /// is released and the watcher's next cycle can re-mind it.
    fn publish(&mut self, ground: &Arc<MachineReport>) {
        let mut dead: Vec<String> = Vec::new();
        for device in self.devices.values_mut() {
            let Some(outbound) = &device.outbound else { continue };
            if !Self::may_stream(&self.roster, device) {
                continue; // the roster has not granted this stream
            }
            if outbound.send(SessionMsg::Out(Outbound::Ground(Arc::clone(ground)))).is_err() {
                dead.push(device.facts.port.clone());
            } else if let Ok(mut t) = device.last_fed.lock() {
                *t = Some(Instant::now());
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
            let Some(outbound) = &device.outbound else { continue };
            if !Self::may_stream(&self.roster, device) {
                continue;
            }
            consumers += 1;
            if outbound
                .send(SessionMsg::Out(Outbound::Pulse { axis: axis.to_string(), value }))
                .is_err()
            {
                dead.push(device.facts.port.clone());
            }
        }
        for port in dead {
            println!("[devices] {port}: consumer died — disposing");
            self.gone(&port);
        }
        if !self.pulse_announced {
            self.pulse_announced = true;
            println!("[devices] pulse lane alive: {axis}={value} across {consumers} consumer(s)");
        }
    }

    /// One in-band shot through the owning session, decoded to PNG.
    /// While a record runs on the port, recording subsumes the
    /// preview: the served frame is the frame the GIF is taking.
    fn capture(&self, port: &str, reply: std_mpsc::SyncSender<Vec<u8>>) {
        if let Some(state) = self.records.get(port) {
            if let Ok(st) = state.lock() {
                if st.is_recording() {
                    let _ = reply.send(st.latest_png.clone().unwrap_or_default());
                    return;
                }
            }
        }
        let png = match self.devices.get(port).and_then(|d| d.outbound.as_ref()) {
            Some(outbound) => {
                let (tx, rx) = std_mpsc::sync_channel(1);
                match outbound.send(SessionMsg::Capture { reply: tx }) {
                    Ok(()) => rx
                        .recv_timeout(Duration::from_secs(10))
                        .unwrap_or_default(),
                    Err(_) => Vec::new(),
                }
            }
            None => Vec::new(), // no live session: an honest empty shot
        };
        let _ = reply.send(png);
    }

    /// One shot, saved into the captures folder. The folder is
    /// `SUZU_CAPTURES_DIR`, or `captures/` beside the resident.
    fn capture_save(&self, port: &str) -> anyhow::Result<String> {
        let (tx, rx) = std_mpsc::sync_channel(1);
        self.capture(port, tx);
        let png = rx
            .recv_timeout(Duration::from_secs(10))
            .map_err(|_| anyhow::anyhow!("{port}: the session answered no shot within 10 s"))?;
        if png.is_empty() {
            anyhow::bail!("{port}: empty shot — face unreachable or frame law missing");
        }
        let dir = std::env::var("SUZU_CAPTURES_DIR").unwrap_or_else(|_| "captures".into());
        std::fs::create_dir_all(&dir)?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let path = format!("{dir}/shot-{port}-{stamp}.png");
        std::fs::write(&path, png)?;
        Ok(path)
    }

    fn record_start(&mut self, port: &str, secs: u32, fps: u32) -> anyhow::Result<()> {
        if let Some(state) = self.records.get(port) {
            if state.lock().map(|s| s.is_recording()).unwrap_or(false) {
                anyhow::bail!("{port}: a recording is already running");
            }
        }
        let state = Arc::new(Mutex::new(RecordState {
            phase: "recording".into(),
            ..Default::default()
        }));
        self.send_to_session(
            port,
            SessionMsg::Record { secs: secs.clamp(1, 60), fps, state: Arc::clone(&state) },
        )
        .map_err(|e| {
            if let Ok(mut s) = state.lock() {
                s.phase = "failed".into();
            }
            e
        })?;
        self.records.insert(port.to_string(), state);
        Ok(())
    }

    fn record_status(&self, port: &str) -> Option<RecordState> {
        self.records
            .get(port)
            .and_then(|s| s.lock().ok())
            .map(|s| s.clone())
    }

    fn admission_retry(&self, port: &str) -> anyhow::Result<()> {
        self.send_to_session(port, SessionMsg::Admission)
    }

    /// Per-device pause: withdraw the subscription but keep the
    /// session — resume is instant and the port is never churned.
    /// The face, hearing nothing, honestly falls to its garden.
    fn pause_device(&mut self, port: &str) -> anyhow::Result<()> {
        let device_id = self
            .devices
            .get(port)
            .and_then(|d| d.device_id().map(|s| s.to_string()))
            .ok_or_else(|| anyhow::anyhow!("{port}: no minded device"))?;
        self.roster
            .write()
            .map_err(|_| anyhow::anyhow!("roster lock poisoned"))?
            .pause(&device_id)
            .map_err(|e| anyhow::anyhow!("{port}: cannot pause ({e:?})"))?;
        if let Some(d) = self.devices.get_mut(port) {
            d.streaming.store(false, Ordering::Relaxed);
        }
        let _ = self.events.send(HouseEvent::StreamDetached {
            device_id,
            port: port.to_string(),
            reason: "paused by the keeper".into(),
        });
        Ok(())
    }

    fn resume_device(&mut self, port: &str) -> anyhow::Result<()> {
        let device_id = self
            .devices
            .get(port)
            .and_then(|d| d.device_id().map(|s| s.to_string()))
            .ok_or_else(|| anyhow::anyhow!("{port}: no minded device"))?;
        self.roster
            .write()
            .map_err(|_| anyhow::anyhow!("roster lock poisoned"))?
            .resume(&device_id)
            .map_err(|e| anyhow::anyhow!("{port}: cannot resume ({e:?})"))?;
        if let Some(d) = self.devices.get_mut(port) {
            d.streaming.store(true, Ordering::Relaxed);
        }
        let _ = self.events.send(HouseEvent::StreamAttached {
            device_id,
            port: port.to_string(),
        });
        Ok(())
    }

    fn send_to_session(&self, port: &str, msg: SessionMsg) -> anyhow::Result<()> {
        let device = self
            .devices
            .get(port)
            .ok_or_else(|| anyhow::anyhow!("{port}: no minded device"))?;
        let outbound = device
            .outbound
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{port}: no live session — is the stream paused?"))?;
        outbound.send(msg).map_err(|_| anyhow::anyhow!("{port}: session died mid-request"))
    }

    /// Hand the individual to a maintenance saga. The session closes
    /// (the port must belong to exactly one master), the saga runs
    /// with the port to itself, and the session respawns afterward —
    /// its admission exam is the gate back into the stream. The command
    /// acks as soon as the saga *begins*; its progress arrives as
    /// MaintenanceStep events and its end as MaintenanceFinished.
    fn maintenance_begin(&mut self, port: &str, kind: &str) -> anyhow::Result<()> {
        // The keeper's verb is "install"; the saga depends on what the
        // face speaks today - an ancestor needs the full install, a
        // suzu face just its files back.
        let speaks_suzu = self
            .devices
            .get(port)
            .and_then(|d| d.facts.proto.as_deref())
            == Some("suzu/1");
        let kind = if kind == "install" && !speaks_suzu {
            "adopt"
        } else {
            kind
        };
        if kind != "install" && kind != "adopt" && kind != "soft" && kind != "factory" {
            anyhow::bail!("unknown maintenance kind {kind:?} - install | adopt | soft | factory");
        }
        if self.in_maintenance.contains_key(port) {
            anyhow::bail!("{port}: a maintenance saga is already running");
        }
        let Some(device) = self.devices.get(port) else {
            anyhow::bail!("{port}: no minded device");
        };
        let Some(device_id) = device.device_id().map(|s| s.to_string()) else {
            anyhow::bail!("{port}: no device_id — adopt the individual first");
        };
        let class = device.facts.class.clone();
        let vid = device.facts.vid;
        let pid = device.facts.pid;

        // Detach the stream for the whole saga, then the port itself.
        let _ = self.events.send(HouseEvent::StreamDetached {
            device_id: device_id.clone(),
            port: port.to_string(),
            reason: format!("maintenance:{kind}"),
        });
        self.close_session(port);
        if let Some(d) = self.devices.get_mut(port) {
            d.outbound = None;
        }
        self.in_maintenance.insert(port.to_string(), kind.to_string());

        let _ = self.events.send(HouseEvent::MaintenanceStarted {
            device_id: device_id.clone(),
            port: port.to_string(),
            kind: kind.to_string(),
        });

        // The saga runs on its own thread (it blocks on tools, drives
        // and human gates); its ending comes back through the door.
        let events = self.events.clone();
        let catalog = Arc::clone(&self.catalog);
        let catalog_fresh = Arc::clone(&self.catalog);
        let device_id2 = device_id.clone();
        let port2 = port.to_string();
        let port3 = port2.clone();
        let kind2 = kind.to_string();
        let door = self.door.clone();
        tokio::spawn(async move {
            let outcome = tokio::task::spawn_blocking(move || {
                super::maintenance::run(
                    &port2, class.as_deref(), &kind2, &catalog, &events, &device_id2,
                )
            })
            .await
            .unwrap_or_else(|e| Err(anyhow::anyhow!("saga panicked: {e}")));
            let ok = outcome.is_ok();
            if let Err(e) = &outcome {
                println!("[maintenance] {port3}: failed — {e:#}");
            }
            let port4 = port3.clone();
            let fresh = tokio::task::spawn_blocking(move || {
                super::watcher::identify_facts(&catalog_fresh, &port4, vid, pid).ok()
            })
            .await
            .unwrap_or(None);
            let _ = door
                .send(DevicesCmd::MaintenanceFinished { port: port3, device_id, ok, fresh })
                .await;
        });
        Ok(())
    }

    /// The saga's end: the verdict lands on the roster's record and
    /// the session respawns — its admission exam is the gate back.
    async fn maintenance_finish(
        &mut self,
        port: &str,
        device_id: &str,
        ok: bool,
        fresh: Option<DeviceFacts>,
    ) {
        let _ = self.events.send(HouseEvent::MaintenanceCompleted {
            device_id: device_id.to_string(),
            kind: self
                .in_maintenance
                .get(port)
                .cloned()
                .unwrap_or_else(|| "unknown".into()),
            ok,
        });
        self.in_maintenance.remove(port);
        if let Some(d) = self.devices.get_mut(port) {
            d.streaming.store(false, Ordering::Relaxed);
        }
        if self.devices.contains_key(port) && !self.paused {
            // The saga may have changed what the face speaks (adopt).
            // Re-identify before respawning, so the session opens with
            // the truth instead of the memory of it.
            let facts = self.devices[port].facts.clone();
            let catalog = Arc::clone(&self.catalog);
            let port2 = port.to_string();
            let fresh = tokio::task::spawn_blocking(move || {
                super::watcher::identify_facts(&catalog, &port2, facts.vid, facts.pid).ok()
            })
            .await
            .unwrap_or(None);
            match fresh {
                Some(new_facts) => {
                    println!("[maintenance] {port}: re-identified as {}/{} — respawning",
                        new_facts.family.as_deref().unwrap_or("?"),
                        new_facts.variant.as_deref().unwrap_or("?"));
                    self.devices.get_mut(port).map(|d| d.facts = new_facts.clone());
                    self.spawn_session(&new_facts);
                }
                None => {
                    println!("[maintenance] {port}: the port went quiet after the saga — replug to re-admit");
                }
            }
        }
    }

    pub fn snapshot(&self) -> Vec<DeviceRow> {
        let roster = self.roster.read().ok();
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
                lifecycle: roster
                    .as_ref()
                    .and_then(|r| {
                        d.device_id().and_then(|id| r.individual(id).map(|i| i.lifecycle))
                    })
                    .map(|l| format!("{l:?}").to_lowercase()),
                streaming: d.streaming.load(Ordering::Relaxed),
                last_data_s: d
                    .last_fed
                    .lock()
                    .ok()
                    .and_then(|t| *t)
                    .map(|t| t.elapsed().as_secs()),
            })
            .collect()
    }
}

// ── the session — one thread per device, one master of the port ────

#[allow(clippy::too_many_arguments)]
fn session_thread(
    port: String,
    rx: std_mpsc::Receiver<SessionMsg>,
    close: Arc<AtomicBool>,
    streaming: Arc<AtomicBool>,
    suzu: bool,
    spec: Option<FrameSpec>,
    zones: Vec<(usize, usize, [u8; 3])>,
    events: Sender<HouseEvent>,
    device_id: Option<String>,
    class: Option<String>,
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

    // The admission exam runs before anything flows. Its verdict goes
    // to the roster; the roster's StreamAttached opens the gate.
    if suzu {
        let report = admission::run(&mut serial, class.as_deref(), spec.as_ref(), &zones);
        let _ = events.send(HouseEvent::AdmissionReport {
            device_id: device_id.clone().unwrap_or_default(),
            port: port.clone(),
            passed: report.passed,
            steps: report.steps,
        });
    }

    // A session that panics releases the port with a name on the way
    // out — never a silent thread death holding hardware hostage.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session_loop(
            &mut serial, &rx, &close, &streaming, suzu, &port, spec, zones,
            &events, device_id, class,
        );
    }));
    if outcome.is_err() {
        println!("[sessions] {port}: session panicked — port released");
    }
    println!("[sessions] {port}: released — fireflies when idle");
}

/// One translated frame to the wire, with a second chance. USB
/// hiccups are transient (one struck ten minutes into a healthy
/// stream); a face that survives the settle keeps its session.
fn write_line_twice(
    serial: &mut Box<dyn SerialPort>,
    port: &str,
    frame: &str,
) -> anyhow::Result<()> {
    if write_line(serial, frame).is_ok() {
        return Ok(());
    }
    std::thread::sleep(Duration::from_millis(1500));
    write_line(serial, frame).map_err(|e| {
        println!("[sessions] {port}: write failed twice — disposing");
        e
    })
}

#[allow(clippy::too_many_arguments)]
fn session_loop(
    serial: &mut Box<dyn SerialPort>,
    rx: &std_mpsc::Receiver<SessionMsg>,
    close: &Arc<AtomicBool>,
    streaming: &Arc<AtomicBool>,
    suzu: bool,
    port: &str,
    spec: Option<FrameSpec>,
    zones: Vec<(usize, usize, [u8; 3])>,
    events: &Sender<HouseEvent>,
    device_id: Option<String>,
    class: Option<String>,
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
            Ok(SessionMsg::Out(Outbound::Ground(g))) => {
                if !streaming.load(Ordering::Relaxed) {
                    continue; // the roster has not granted this stream
                }
                let frames = if suzu {
                    translate_suzu(&g, &mut named)
                } else {
                    translate(&g)
                };
                for frame in frames {
                    if write_line_twice(serial, port, &frame).is_err() {
                        return;
                    }
                }
            }
            Ok(SessionMsg::Out(Outbound::Pulse { axis, value })) => {
                if !suzu || !streaming.load(Ordering::Relaxed) {
                    continue;
                }
                let frame = format!("A,{axis},{value}");
                if write_line_twice(serial, port, &frame).is_err() {
                    return;
                }
            }
            Ok(SessionMsg::Out(Outbound::Ring { signal, words, urgency })) => {
                if !suzu || !streaming.load(Ordering::Relaxed) {
                    continue;
                }
                seq = seq.wrapping_add(1);
                let mut frame = format!("R,{signal},{urgency},0,1,{seq},{}", words.join(","));
                frame = with_checksum(&frame);
                if write_line_twice(serial, port, &frame).is_err() {
                    return;
                }
            }
            Ok(SessionMsg::Capture { reply }) => {
                let png = match (&spec, suzu) {
                    (Some(spec), true) => {
                        crate::shot::capture_on(serial, spec.size)
                            .and_then(|frame| crate::shot::render_png_bytes(spec, &zones, &frame))
                            .unwrap_or_default()
                    }
                    _ => Vec::new(),
                };
                let _ = reply.send(png);
            }
            Ok(SessionMsg::Record { secs, fps, state }) => {
                record_job(serial, port, secs, fps, spec.as_ref(), &zones, &state);
            }
            Ok(SessionMsg::Admission) => {
                if suzu {
                    let report = admission::run(serial, class.as_deref(), spec.as_ref(), &zones);
                    let _ = events.send(HouseEvent::AdmissionReport {
                        device_id: device_id.clone().unwrap_or_default(),
                        port: port.to_string(),
                        passed: report.passed,
                        steps: report.steps,
                    });
                }
            }
            Ok(SessionMsg::Close) => break,
            // Keepalive: a suzu face rests after 10 s of silence, the
            // ancestor idles to its fireflies — a frame every 5 s
            // holds either face. Only while the stream flows; an
            // ungranted face rests honestly.
            Err(std_mpsc::RecvTimeoutError::Timeout) => {
                if !streaming.load(Ordering::Relaxed) {
                    continue;
                }
                let keepalive = if suzu { "K" } else { "R" };
                let _ = write_line(serial, keepalive);
            }
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// The trail camera, on the session's own port: loop the in-band shot,
/// keep the latest frame served (recording subsumes the preview), and
/// assemble the GIF into the captures folder when the run ends.
fn record_job(
    serial: &mut Box<dyn SerialPort>,
    port: &str,
    secs: u32,
    fps: u32,
    spec: Option<&FrameSpec>,
    zones: &[(usize, usize, [u8; 3])],
    state: &Mutex<RecordState>,
) {
    let Some(spec) = spec else {
        if let Ok(mut s) = state.lock() {
            s.phase = "done".into();
        }
        return;
    };
    let fps = fps.clamp(1, 5);
    let period = Duration::from_millis(1000 / fps as u64);
    let delay_cs = ((1000 / fps as u16) / 10).max(2);

    if let Ok(mut s) = state.lock() {
        s.phase = "recording".into();
        s.frames = 0;
        s.gif_path = None;
    }

    let mut rgba_frames: Vec<Vec<u8>> = Vec::new();
    let (mut vw, mut vh) = (0usize, 0usize);
    let mut next_at = Instant::now();
    let end = next_at + Duration::from_secs(secs as u64);
    let mut quiet = false;
    while Instant::now() < end {
        next_at += period;
        match crate::shot::capture_on(serial, spec.size) {
            Ok(frame) => match crate::shot::render_view(spec, zones, &frame) {
                Ok((w, h, rgba)) => {
                    let scale = spec.render.as_ref().map(|r| r.scale).unwrap_or(1).max(1);
                    let scaled = scale_rgba(&rgba, w, h, scale);
                    let png = {
                        let rgb: Vec<[u8; 3]> =
                            scaled.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
                        crate::shot::png_bytes(w * scale, h * scale, &rgb, 1).unwrap_or_default()
                    };
                    vw = w * scale;
                    vh = h * scale;
                    rgba_frames.push(scaled);
                    if let Ok(mut s) = state.lock() {
                        s.frames = rgba_frames.len();
                        s.latest_png = Some(png);
                    }
                }
                Err(_) => {}
            },
            Err(_) => {
                quiet = true;
                break;
            }
        }
        let now = Instant::now();
        if next_at > now {
            std::thread::sleep(next_at - now);
        } else {
            next_at = now; // wire-bound: skip the missed slot, keep going
        }
    }

    let mut finished = RecordState {
        phase: if quiet { "failed".into() } else { "done".into() },
        frames: rgba_frames.len(),
        gif_path: None,
        latest_png: None,
    };
    if !rgba_frames.is_empty() && vw > 0 {
        let dir = std::env::var("SUZU_CAPTURES_DIR").unwrap_or_else(|_| "captures".into());
        let _ = std::fs::create_dir_all(&dir);
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let path = format!("{dir}/record-{port}-{stamp}.gif");
        match crate::gif::write_gif_rgba(
            std::path::Path::new(&path), vw, vh, delay_cs, &rgba_frames,
        ) {
            Ok(()) => finished.gif_path = Some(path),
            Err(e) => println!("[sessions] {port}: gif assembly failed — {e}"),
        }
    }
    if let Ok(mut s) = state.lock() {
        *s = finished;
    }
}

fn scale_rgba(rgba: &[u8], w: usize, h: usize, scale: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len() * scale * scale);
    for y in 0..h {
        for _ in 0..scale {
            for x in 0..w {
                let px = &rgba[(y * w + x) * 4..(y * w + x) * 4 + 4];
                for _ in 0..scale {
                    out.extend_from_slice(px);
                }
            }
        }
    }
    out
}

fn translate_suzu(g: &MachineReport, named: &mut Option<String>) -> Vec<String> {
    let mut frames = Vec::new();
    if named.as_deref() != Some(g.name.as_str()) {
        frames.push(format!("J,{{\"name\":\"{}\"}}", g.name.replace('"', "'")));
        *named = Some(g.name.clone());
    }
    frames.push(format!("G,report,{},{},{}", g.cpu, g.mem, g.gpu.unwrap_or(255)));
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

fn open_serial(port: &str) -> anyhow::Result<Box<dyn SerialPort>> {
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

fn write_line(serial: &mut Box<dyn SerialPort>, line: &str) -> anyhow::Result<()> {
    serial.write_all(line.as_bytes())?;
    serial.write_all(b"\n")?;
    serial.flush()?;
    Ok(())
}

fn now() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

//! The devices domain — the manager of minded devices, and the
//! consumers of published ground.
//!
//! Each live device owns a session thread that owns its port
//! exclusively: stream translation, the frame lane, the trail-camera
//! record, and the admission test all ride that one thread, because a
//! serial port answers one master. Whether the stream *flows* is not
//! this domain's decision — the roster grants the subscription
//! (ADR-0003), and this domain merely obeys the gate.
//!
//! The actor's law (ADR-0004): **the loop only routes.** Nothing here
//! waits on a face — a session answers through its mailbox, the frame
//! lane publishes on the house's own cadence, and a stuck face
//! degrades only that face. The read model rides the wire whole:
//! whenever the rows change, one `Devices` fact replaces every
//! client's slice.

use super::admission;
use super::events::{DeviceFacts, DeviceRow, FrameFacts, HouseEvent};
use super::jobs::{Job, Jobs};
use super::roster::Roster;
use super::sensor::MachineReport;
use crate::catalog::{Catalog, DisplayZones, FrameSpec};
use serde::Serialize;
use serialport::SerialPort;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc as std_mpsc, Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::{broadcast::Sender, mpsc};

/// The media lane's cadence, house-enforced: at most one capture in
/// flight per face (the session thread is serial by construction) and
/// one blink per `FRAME_PERIOD` — no client cadence can flood the wire
/// because the client commands nothing about *how* the lane runs
/// (ADR-0004). The lane is *watched* (the amendment): it blinks only
/// while a window asserts the watch.
const FRAME_PERIOD: Duration = Duration::from_secs(2);
/// A frame older than this is not a frame: the shot door fails
/// honestly instead of serving a memory of the face.
const MAX_FRAME_AGE: Duration = Duration::from_secs(5);
/// The session mailbox is bounded: a session that cannot keep up is a
/// stuck session, and a full mailbox disposes it loudly (law: every
/// channel has capacity, every overload a journal line).
const SESSION_MAILBOX: usize = 64;
/// The session's keepalive beat: a suzu face rests after 10 s of
/// silence, the ancestor idles to its fireflies — a frame every 5 s
/// holds either face.
const KEEPALIVE_PERIOD: Duration = Duration::from_secs(5);

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
    /// The session mailbox — this device's consumer. Bounded: a full
    /// mailbox is a stuck session, not a queue to grow forever.
    #[serde(skip)]
    pub outbound: Option<std_mpsc::SyncSender<SessionMsg>>,
    /// The roster's gate, mirrored here for the session thread to read
    /// at wire speed. The roster is the truth; this is the echo.
    #[serde(skip)]
    pub streaming: Arc<AtomicBool>,
    /// When the face last heard from the house — the honest aliveness
    /// signal ("spoke 4s ago") beats any checklist.
    #[serde(skip)]
    pub last_fed: Arc<Mutex<Option<Instant>>>,
    /// Whether this face's session can blink frames at all (suzu wire,
    /// frame law declared) — the watched-lane count's denominator.
    #[serde(skip)]
    pub blinks: bool,
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
    /// The trail camera: exclusive on the session until done. The
    /// registry is the record — progress and verdict travel as Job
    /// facts, and the frames the GIF takes ride the frame lane.
    Record { job_id: String, secs: u32, fps: u32 },
    /// Re-run the admission exam (roster decides what it means).
    Admission,
    Close,
}

pub enum Outbound {
    Ground(Arc<MachineReport>),
    Pulse { axis: String, value: u8 },
    Ring { signal: String, words: Vec<String>, urgency: u8 },
}

/// The devices read model plus the service facts the door needs — one
/// round trip for the snapshot fact (ADR-0004).
#[derive(Debug, Clone, Serialize)]
pub struct DevicesSnapshot {
    pub devices: Vec<DeviceRow>,
    pub paused: bool,
    pub media_watched: bool,
    pub frames: Vec<FrameFacts>,
}

/// The watched lane's verdict, for the door's reply: whether the flag
/// moved, and how many faces are blinking now.
#[derive(Debug, Clone, Serialize)]
pub struct WatchReport {
    pub changed: bool,
    pub blinking: usize,
}

/// The stream toggle's verdict, for the door's reply: whether the
/// pause flag moved, and how many minded ports the stream touches.
#[derive(Debug, Clone, Serialize)]
pub struct StreamReport {
    pub changed: bool,
    pub ports: usize,
}

pub enum DevicesCmd {
    Mind(DeviceFacts),
    Gone { port: String },
    /// The publisher's outbound pipeline: one call, every live consumer.
    Publish(Arc<MachineReport>),
    Pulse { axis: String, value: u8 },
    /// The read model, taken by the owning domain. Answers at once —
    /// the loop routes, it never waits.
    Snapshot { reply: mpsc::Sender<DevicesSnapshot> },
    /// The newest frame for a port, under the freshness bound. An
    /// honest error when the face has not blinked lately.
    LatestFrame { port: String, reply: mpsc::Sender<anyhow::Result<Vec<u8>>> },
    /// Save the newest frame into the captures folder; replies the path.
    CaptureSave { port: String, reply: mpsc::Sender<anyhow::Result<String>> },
    /// The control chirp: stop streaming and release the ports (the
    /// faces fall idle into their animations), then re-open and
    /// re-publish. In-memory only — it dies with the process.
    Pause { reply: mpsc::Sender<StreamReport> },
    Resume { reply: mpsc::Sender<StreamReport> },
    /// The watched lane (ADR-0004 amendment): a window asserted (or
    /// released) its watch on the media lane. The reply is optional —
    /// the wire's own release on last-client-departure needs no ack.
    WatchMedia { on: bool, reply: Option<mpsc::Sender<WatchReport>> },
    /// A moment bound for faces: the band shows the label briefly;
    /// the signal names an icon when the face has one.
    Ring { signal: String, label: String, urgency: u8 },
    /// The trail camera, on the owning session. Acks whether the
    /// session took the job; the verdict travels as Job facts.
    RecordStart { port: String, job_id: String, secs: u32, fps: u32, reply: mpsc::Sender<anyhow::Result<()>> },
    /// Re-run the admission exam through the owning session.
    AdmissionRetry { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    /// The keeper lifted one device off the stream (per-device pause):
    /// the gate closes, the session stays, the face falls to its
    /// garden. Resume re-subscribes without a re-test.
    /// The keeper points at one face among twins: "this is me".
    /// A targeted ring — one session, no budget, no broadcast.
    Identify { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    PauseDevice { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    ResumeDevice { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    /// Hand the individual to a maintenance saga: the session closes,
    /// the port goes to the saga, the stream returns only after the
    /// saga's admission test passes. `faceplate` names the dress for
    /// classes that declare them (ADR-0005); None keeps the default.
    MaintenanceStart {
        port: String,
        kind: String,
        faceplate: Option<String>,
        reply: mpsc::Sender<anyhow::Result<()>>,
    },
    /// The saga's own end — it loops back through the door so the
    /// devices domain (which owns the sessions) respawns the face.
    /// The saga's task re-identifies off-loop and its facts ride in.
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
    jobs: Arc<Jobs>,
    /// The frame lane's cache: the newest frame per port, and when it
    /// was taken. The shot doors read it; freshness is the truth test.
    frames: BTreeMap<String, (Instant, String)>,
    /// port → (kind, faceplate) while a saga runs (ADR-0005).
    in_maintenance: BTreeMap<String, (String, Option<String>)>,
    pulse_announced: bool,
    paused: bool,
    /// The watched lane's echo (ADR-0004 amendment): the gate's flag,
    /// read by the session threads at wire speed.
    media_watched: Arc<AtomicBool>,
    rows_dirty: bool,
}

struct SessionHandle {
    close: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Devices {
    pub fn new(
        events: Sender<HouseEvent>,
        door: mpsc::Sender<DevicesCmd>,
        catalog: Arc<Catalog>,
        roster: Arc<std::sync::RwLock<Roster>>,
        jobs: Arc<Jobs>,
    ) -> Self {
        Self {
            events,
            door,
            roster,
            catalog,
            devices: BTreeMap::new(),
            sessions: BTreeMap::new(),
            jobs,
            frames: BTreeMap::new(),
            in_maintenance: BTreeMap::new(),
            pulse_announced: false,
            paused: false,
            media_watched: Arc::new(AtomicBool::new(false)),
            rows_dirty: false,
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
                            let snap = self.devices_snapshot();
                            let _ = reply.send(snap).await;
                        }
                        DevicesCmd::LatestFrame { port, reply } => {
                            let res = self.latest_frame(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::CaptureSave { port, reply } => {
                            let res = self.capture_save(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::Pause { reply } => {
                            let report = self.pause_stream();
                            let _ = reply.send(report).await;
                        }
                        DevicesCmd::Resume { reply } => {
                            let report = self.resume_stream().await;
                            let _ = reply.send(report).await;
                        }
                        DevicesCmd::WatchMedia { on, reply } => {
                            let report = self.watch_media(on);
                            if let Some(reply) = reply {
                                let _ = reply.send(report).await;
                            }
                        }
                        DevicesCmd::Ring { signal, label, urgency } => {
                            if !self.paused { self.ring(&signal, &label, urgency) }
                        }
                        DevicesCmd::RecordStart { port, job_id, secs, fps, reply } => {
                            let res = self.record_start(&port, &job_id, secs, fps);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::AdmissionRetry { port, reply } => {
                            let res = self.admission_retry(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::Identify { port, reply } => {
                            let res = self.identify(&port);
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
                        DevicesCmd::MaintenanceStart { port, kind, faceplate, reply } => {
                            let res = self.maintenance_begin(&port, &kind, faceplate);
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
                                self.rows_dirty = true;
                            }
                        }
                        Ok(HouseEvent::StreamDetached { port, .. }) => {
                            if let Some(d) = self.devices.get_mut(&port) {
                                d.streaming.store(false, Ordering::Relaxed);
                                self.rows_dirty = true;
                            }
                        }
                        // The frame lane loops back through the bus: the
                        // session publishes, the actor caches for the
                        // shot doors and the snapshot fact.
                        Ok(HouseEvent::Frame { port, png }) => {
                            self.frames.insert(port, (Instant::now(), png));
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
            }
            // The read model rides the wire whole: one fact replaces
            // every client's slice whenever the rows changed.
            if self.rows_dirty {
                self.rows_dirty = false;
                let rows = self.snapshot();
                let _ = self.events.send(HouseEvent::Devices { rows });
            }
        }
    }

    /// Classes with a known consumer translation. Others are minded but
    /// stay silent until their dialect is codified.
    fn supports_consumer(class: Option<&str>) -> bool {
        class == Some("esp8266-oled-v2-class")
    }

    fn frame_law_of(&self, facts: &DeviceFacts) -> (Option<FrameSpec>, crate::catalog::DisplayZones) {
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
        // gone, resume after resume) is closed and left to exit; the
        // new session's open retries until the port is free. The loop
        // never joins a thread.
        let joining = self.close_session(&facts.port);
        if let Some(join) = joining {
            let port = facts.port.clone();
            std::thread::Builder::new()
                .name(format!("reaper:{port}"))
                .spawn(move || {
                    let _ = join.join();
                })
                .ok();
        }
        let (tx, rx) = std_mpsc::sync_channel::<SessionMsg>(SESSION_MAILBOX);
        let (spec, zones) = self.frame_law_of(facts);
        let blinks = suzu && spec.is_some();
        let streaming = Arc::new(AtomicBool::new(false));
        let close = Arc::new(AtomicBool::new(false));
        let port = facts.port.clone();
        let events = self.events.clone();
        let jobs = Arc::clone(&self.jobs);
        let media_watched = Arc::clone(&self.media_watched);
        let device_id = facts.device_id.clone();
        let class = facts.class.clone();
        let streaming2 = Arc::clone(&streaming);
        let close2 = Arc::clone(&close);
        let join = std::thread::Builder::new()
            .name(format!("session:{port}"))
            .spawn(move || {
                session_thread(
                    port, rx, close2, streaming2, suzu, spec, zones, events, jobs,
                    media_watched, device_id, class,
                )
            })
            .ok();
        self.sessions
            .insert(facts.port.clone(), SessionHandle { close, join });
        if let Some(device) = self.devices.get_mut(&facts.port) {
            device.outbound = Some(tx);
            device.streaming = streaming;
            device.blinks = blinks;
        }
        self.rows_dirty = true;
    }

    /// Close a session and hand back its thread, if it has one. The
    /// caller joins only when it must (a saga needs the port to
    /// itself); everyone else lets the thread exit on its own — the
    /// close flag and the Close message wake it within seconds.
    fn close_session(&mut self, port: &str) -> Option<std::thread::JoinHandle<()>> {
        let mut handle = self.sessions.remove(port)?;
        handle.close.store(true, Ordering::Relaxed);
        if let Some(outbound) = self.devices.get_mut(port) {
            if let Some(out) = outbound.outbound.take() {
                let _ = out.send(SessionMsg::Close);
            }
            outbound.streaming.store(false, Ordering::Relaxed);
        }
        self.frames.remove(port);
        handle.join.take()
    }

    /// A ring: every live session tells its face that something
    /// happened. The frame carries the moment's words after the seq.
    fn ring(&mut self, signal: &str, label: &str, urgency: u8) {
        let words: Vec<String> = label.split_whitespace().map(|s| s.to_string()).collect();
        for device in self.devices.values_mut() {
            if let Some(outbound) = &device.outbound {
                let _ = outbound.try_send(SessionMsg::Out(Outbound::Ring {
                    signal: signal.to_string(),
                    words: words.clone(),
                    urgency,
                }));
            }
        }
    }

    /// The watched lane's gate (ADR-0004 amendment): the window's
    /// assertion arms the blink; the wire's liveness holds it. Idempotent
    /// — every Media entry re-asserts, and repeats cost nothing.
    fn watch_media(&mut self, on: bool) -> WatchReport {
        let changed = self.media_watched.swap(on, Ordering::Relaxed) != on;
        if changed {
            let _ = self.events.send(HouseEvent::MediaWatched { watched: on });
        }
        WatchReport {
            changed,
            blinking: self
                .devices
                .values()
                .filter(|d| d.blinks && d.streaming.load(Ordering::Relaxed))
                .count(),
        }
    }

    /// Pause: sessions close, ports release, the ground stops. The
    /// faces fall idle into their animations; the devices stay minded
    /// so `resume` re-opens without replug.
    fn pause_stream(&mut self) -> StreamReport {
        if self.paused {
            return StreamReport { changed: false, ports: self.devices.len() };
        }
        self.paused = true;
        let ports: Vec<String> = self.devices.keys().cloned().collect();
        for port in &ports {
            self.close_session(port);
            if let Some(device) = self.devices.get_mut(port) {
                device.outbound = None;
            }
        }
        self.rows_dirty = true;
        let _ = self.events.send(HouseEvent::Paused { paused: true });
        println!(
            "[devices] stream paused — {} port(s) released, faces fall idle (`suzu resume` to restart)",
            self.devices.len()
        );
        StreamReport { changed: true, ports: self.devices.len() }
    }

    /// Resume: sessions re-open (each re-taking its admission exam),
    /// and the publisher republishes its last ground so admitted faces
    /// redress at once.
    async fn resume_stream(&mut self) -> StreamReport {
        if !self.paused {
            return StreamReport { changed: false, ports: self.devices.len() };
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
        let _ = self.events.send(HouseEvent::Paused { paused: false });
        StreamReport { changed: true, ports: self.devices.len() }
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
                blinks: false,
            },
        );
        self.rows_dirty = true;
        // A paused house stays silent: the session spawns on resume.
        if !self.paused {
            self.spawn_session(&facts);
        }
    }

    fn gone(&mut self, port: &str) {
        if let Some(mut device) = self.devices.remove(port) {
            self.close_session(port);
            device.outbound = None;
            self.rows_dirty = true;
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
    /// mailbox is gone or full (its thread died or stalled) is
    /// disposed here — the port is released and the watcher's next
    /// cycle can re-mind it.
    fn publish(&mut self, ground: &Arc<MachineReport>) {
        let mut dead: Vec<String> = Vec::new();
        for device in self.devices.values_mut() {
            let Some(outbound) = &device.outbound else { continue };
            if !Self::may_stream(&self.roster, device) {
                continue; // the roster has not granted this stream
            }
            if let Err(e) = outbound.try_send(SessionMsg::Out(Outbound::Ground(Arc::clone(ground)))) {
                if matches!(e, std_mpsc::TrySendError::Disconnected(_)) {
                    dead.push(device.facts.port.clone());
                }
            } else if let Ok(mut t) = device.last_fed.lock() {
                *t = Some(Instant::now());
            }
        }
        for port in dead {
            println!("[devices] {port}: consumer died — disposing");
            self.gone(&port);
        }
        self.rows_dirty = true;
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
            if let Err(e) = outbound.try_send(SessionMsg::Out(Outbound::Pulse { axis: axis.to_string(), value }))
                && matches!(e, std_mpsc::TrySendError::Disconnected(_)) {
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

    /// The newest frame for a port, under the freshness bound. This is
    /// the whole capture story (ADR-0004): the house blinks on its own
    /// cadence, the door serves the cached truth, and a stuck face
    /// fails honestly in bounded time — here, instantly.
    fn latest_frame(&self, port: &str) -> anyhow::Result<Vec<u8>> {
        let Some((at, png_b64)) = self.frames.get(port) else {
            anyhow::bail!("{port}: no frame yet — the face has not blinked");
        };
        let age = at.elapsed();
        if age > MAX_FRAME_AGE {
            anyhow::bail!(
                "{port}: the face last blinked {}s ago — unreachable or stuck",
                age.as_secs()
            );
        }
        Ok(crate::shot::decode_b64(png_b64))
    }

    /// One shot, saved into the captures folder. The folder is
    /// `SUZU_CAPTURES_DIR`, or `captures/` beside the resident.
    fn capture_save(&self, port: &str) -> anyhow::Result<String> {
        let png = self.latest_frame(port)?;
        let dir = captures_dir();
        std::fs::create_dir_all(&dir)?;
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let path = format!("{dir}/shot-{port}-{stamp}.png");
        std::fs::write(&path, png)?;
        Ok(path)
    }

    /// The trail camera: hand the job to the owning session and ack.
    /// The registry is the record — progress and the GIF's verdict
    /// travel as Job facts; the ack only says whether the session
    /// took the job.
    fn record_start(&mut self, port: &str, job_id: &str, secs: u32, fps: u32) -> anyhow::Result<()> {
        let secs = secs.clamp(1, 60);
        let fps = fps.clamp(1, 5);
        if let Err(e) = self.send_to_session(port, SessionMsg::Record { job_id: job_id.to_string(), secs, fps }) {
            self.jobs.with(job_id, |j: &mut Job| {
                j.state = "failed".into();
                j.label = format!("{e:#}");
            });
            return Err(e);
        }
        Ok(())
    }

    fn admission_retry(&self, port: &str) -> anyhow::Result<()> {
        self.send_to_session(port, SessionMsg::Admission)
    }

    /// The keeper points at one face: it rings its own name on the
    /// band for a few seconds. A targeted ring rides straight to the
    /// owning session — the moments budget governs visitors, not the
    /// keeper pointing at hardware.
    fn identify(&mut self, port: &str) -> anyhow::Result<()> {
        let device = self
            .devices
            .get(port)
            .ok_or_else(|| anyhow::anyhow!("{port}: no minded device"))?;
        if !device.streaming.load(Ordering::Relaxed) {
            anyhow::bail!("{port}: is not on the stream — only a live face can hear its name");
        }
        self.send_to_session(
            port,
            SessionMsg::Out(Outbound::Ring {
                signal: "info".to_string(),
                words: vec![port.to_string()],
                urgency: 3,
            }),
        )
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
        {
            let mut roster = self
                .roster
                .write()
                .map_err(|_| anyhow::anyhow!("roster lock poisoned"))?;
            let current = roster
                .individual(&device_id)
                .map(|i| format!("{:?}", i.lifecycle).to_lowercase());
            roster.pause(&device_id).map_err(|e| {
                anyhow::anyhow!("{port}: cannot pause — {}", match e {
                    super::roster::Refusal::NotFrom(from) => format!(
                        "that move is only from {from} (this face is {})",
                        current.as_deref().unwrap_or("unknown")
                    ),
                    super::roster::Refusal::Unknown => {
                        "the roster holds no such individual".to_string()
                    }
                })
            })?;
        }
        if let Some(d) = self.devices.get_mut(port) {
            d.streaming.store(false, Ordering::Relaxed);
        }
        self.rows_dirty = true;
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
        {
            let mut roster = self
                .roster
                .write()
                .map_err(|_| anyhow::anyhow!("roster lock poisoned"))?;
            let current = roster
                .individual(&device_id)
                .map(|i| format!("{:?}", i.lifecycle).to_lowercase());
            roster.resume(&device_id).map_err(|e| {
                anyhow::anyhow!("{port}: cannot resume — {}", match e {
                    super::roster::Refusal::NotFrom(from) => format!(
                        "that move is only from {from} (this face is {})",
                        current.as_deref().unwrap_or("unknown")
                    ),
                    super::roster::Refusal::Unknown => {
                        "the roster holds no such individual".to_string()
                    }
                })
            })?;
        }
        if let Some(d) = self.devices.get_mut(port) {
            d.streaming.store(true, Ordering::Relaxed);
        }
        self.rows_dirty = true;
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
        outbound.try_send(msg).map_err(|e| match e {
            std_mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("{port}: the session mailbox is full — the face is stuck")
            }
            std_mpsc::TrySendError::Disconnected(_) => {
                anyhow::anyhow!("{port}: session died mid-request")
            }
        })
    }

    /// Hand the individual to a maintenance saga. The session closes
    /// (the port must belong to exactly one master), the saga runs
    /// with the port to itself, and the session respawns afterward —
    /// its admission exam is the gate back into the stream. The command
    /// acks as soon as the saga *begins*; its progress arrives as
    /// MaintenanceStep events and its end as MaintenanceFinished.
    fn maintenance_begin(
        &mut self,
        port: &str,
        kind: &str,
        faceplate: Option<String>,
    ) -> anyhow::Result<()> {
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
        // The dress must be one the class declares — refused by name,
        // with the vocabulary, per the door contract (ADR-0005).
        if let Some(dress) = &faceplate {
            let declared = class
                .as_deref()
                .map(|c| self.catalog.faceplates_for_class(c))
                .unwrap_or_default();
            if !declared.iter().any(|f| &f.id == dress) {
                let vocab = declared
                    .iter()
                    .map(|f| format!("{:?}", f.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "unknown faceplate {dress:?} — this class declares: {}",
                    if vocab.is_empty() { "none".to_string() } else { vocab }
                );
            }
        }
        let vid = device.facts.vid;
        let pid = device.facts.pid;

        // Detach the stream for the whole saga, then the port itself.
        let _ = self.events.send(HouseEvent::StreamDetached {
            device_id: device_id.clone(),
            port: port.to_string(),
            reason: format!("maintenance:{kind}"),
        });
        // The saga needs the port to itself: join the retired session's
        // thread before the tools open the port — on the saga's own
        // task, never on this loop.
        let joining = self.close_session(port);
        if let Some(d) = self.devices.get_mut(port) {
            d.outbound = None;
        }
        self.in_maintenance
            .insert(port.to_string(), (kind.to_string(), faceplate.clone()));
        self.rows_dirty = true;

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
        let faceplate2 = faceplate.clone();
        let door = self.door.clone();
        tokio::spawn(async move {
            if let Some(join) = joining {
                let _ = tokio::task::spawn_blocking(move || join.join()).await;
            }
            let outcome = tokio::task::spawn_blocking(move || {
                super::maintenance::run(
                    &port2, class.as_deref(), &kind2, &catalog, &events, &device_id2,
                    faceplate2.as_deref(),
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

    /// The saga's end: the verdict lands on the roster's record. The
    /// saga's task already re-identified the face off-loop — its facts
    /// ride in, and the session respawns with the truth instead of the
    /// memory of it. Its admission exam is the gate back.
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
                .map(|(kind, _)| kind.clone())
                .unwrap_or_else(|| "unknown".into()),
            ok,
        });
        self.in_maintenance.remove(port);
        if let Some(d) = self.devices.get_mut(port) {
            d.streaming.store(false, Ordering::Relaxed);
        }
        self.rows_dirty = true;
        match fresh {
            Some(new_facts) if self.devices.contains_key(port) && !self.paused => {
                self.session_respawn(port, new_facts);
            }
            Some(_) => {} // gone or paused meanwhile: the respawn waits
            None => {
                if self.devices.contains_key(port) {
                    println!("[maintenance] {port}: the port went quiet after the saga — replug to re-admit");
                }
            }
        }
    }

    /// A saga's respawn, re-identified off-loop: the record is updated
    /// with the truth and the session opens with its admission exam.
    fn session_respawn(&mut self, port: &str, facts: DeviceFacts) {
        let Some(d) = self.devices.get_mut(port) else {
            return; // the device departed while the ladder ran
        };
        println!(
            "[maintenance] {port}: re-identified as {}/{} — respawning",
            facts.family.as_deref().unwrap_or("?"),
            facts.variant.as_deref().unwrap_or("?")
        );
        d.facts = facts.clone();
        if !self.paused {
            self.spawn_session(&facts);
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

    /// The read model, whole: rows, the pause flag, the watch flag,
    /// the frame cache.
    fn devices_snapshot(&self) -> DevicesSnapshot {
        DevicesSnapshot {
            devices: self.snapshot(),
            paused: self.paused,
            media_watched: self.media_watched.load(Ordering::Relaxed),
            frames: self
                .frames
                .iter()
                .map(|(port, (_, png))| FrameFacts { port: port.clone(), png: png.clone() })
                .collect(),
        }
    }
}

/// The captures folder: `SUZU_CAPTURES_DIR`, or `captures/` beside the
/// resident. One name, one place.
fn captures_dir() -> String {
    std::env::var("SUZU_CAPTURES_DIR").unwrap_or_else(|_| "captures".into())
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
    zones: DisplayZones,
    events: Sender<HouseEvent>,
    jobs: Arc<Jobs>,
    media_watched: Arc<AtomicBool>,
    device_id: Option<String>,
    class: Option<String>,
) {
    // One master per port, with grace: a retired session's thread may
    // still be exiting (a capture takes up to 8 s), so the open
    // retries briefly before declaring the face unreachable.
    let mut serial = None;
    for attempt in 0..12 {
        if close.load(Ordering::Relaxed) {
            return; // retired before it ever opened — the port is free
        }
        match open_serial(&port) {
            Ok(p) => {
                serial = Some(p);
                break;
            }
            Err(e) => {
                if attempt == 11 {
                    println!("[sessions] {port}: open failed — {e} (device stays idle)");
                    return; // rx dropped → the device is simply silent
                }
                std::thread::sleep(Duration::from_millis(500));
            }
        }
    }
    let mut serial = serial.expect("the retry loop either opened or returned");
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
            &events, &jobs, &media_watched, device_id, class,
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
    write_line(serial, frame).inspect_err(|_| {
        println!("[sessions] {port}: write failed twice — disposing");
    })
}

/// One blink of the frame lane: capture, render, publish. A face that
/// misses a blink costs nothing but the attempt — the freshness bound
/// on the shot doors is the honest voice for it.
fn frame_blink(
    serial: &mut Box<dyn SerialPort>,
    port: &str,
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
    events: &Sender<HouseEvent>,
) {
    let png = crate::shot::capture_on(serial, spec.size)
        .and_then(|frame| crate::shot::render_png_bytes(spec, zones, &frame));
    if let Ok(png) = png {
        let _ = events.send(HouseEvent::Frame {
            port: port.to_string(),
            png: crate::shot::encode_b64(&png),
        });
    }
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
    zones: DisplayZones,
    events: &Sender<HouseEvent>,
    jobs: &Jobs,
    media_watched: &Arc<AtomicBool>,
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
    let mut next_frame = Instant::now() + FRAME_PERIOD;
    let mut last_keepalive = Instant::now();

    loop {
        if close.load(Ordering::Relaxed) {
            break;
        }
        // Wake for the nearest of: a message, the next blink, the
        // keepalive. The frame lane is this session's own cadence —
        // serial by construction, so captures coalesce for free.
        let now = Instant::now();
        let till_frame = next_frame.saturating_duration_since(now);
        let since_keepalive = now.duration_since(last_keepalive);
        let till_keepalive = KEEPALIVE_PERIOD
            .checked_sub(since_keepalive)
            .unwrap_or(Duration::ZERO);
        let wait = till_frame.min(till_keepalive);
        match rx.recv_timeout(wait) {
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
            Ok(SessionMsg::Record { job_id, secs, fps }) => {
                record_job(serial, port, &job_id, secs, fps, spec.as_ref(), &zones, jobs, events);
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
            Err(std_mpsc::RecvTimeoutError::Timeout) => {}
            Err(std_mpsc::RecvTimeoutError::Disconnected) => break,
        }
        let now = Instant::now();
        // The media lane's blink, only for someone's eyes: the watch
        // flag (ADR-0004 amendment) rides beside the roster's stream
        // gate. A recording is work, not a glance — its frames publish
        // from inside its own handler, watched or not.
        if now >= next_frame {
            next_frame = now + FRAME_PERIOD;
            if suzu
                && streaming.load(Ordering::Relaxed)
                && media_watched.load(Ordering::Relaxed)
                && let Some(spec) = &spec
            {
                frame_blink(serial, port, spec, &zones, events);
            }
        }
        // Keepalive: only while the stream flows (see FRAME_PERIOD note).
        if now.duration_since(last_keepalive) >= KEEPALIVE_PERIOD {
            last_keepalive = now;
            if streaming.load(Ordering::Relaxed) {
                let keepalive = if suzu { "K" } else { "R" };
                let _ = write_line(serial, keepalive);
            }
        }
    }
}

/// The trail camera, on the session's own port: loop the in-band shot,
/// publish every frame taken (recording subsumes the preview — the
/// frames the GIF takes are the frames the wire carries), and assemble
/// the GIF into the captures folder when the run ends. The registry is
/// the record: progress and verdict travel as Job facts.
#[allow(clippy::too_many_arguments)] // the session's hands, all needed
fn record_job(
    serial: &mut Box<dyn SerialPort>,
    port: &str,
    job_id: &str,
    secs: u32,
    fps: u32,
    spec: Option<&FrameSpec>,
    zones: &[(usize, usize, [u8; 3])],
    jobs: &Jobs,
    events: &Sender<HouseEvent>,
) {
    let Some(spec) = spec else {
        jobs.with(job_id, |j| {
            j.state = "failed".into();
            j.label = "the class declares no frame law".into();
        });
        return;
    };
    let fps = fps.clamp(1, 5);
    let period = Duration::from_millis(1000 / fps as u64);
    let delay_cs = ((1000 / fps as u16) / 10).max(2);

    jobs.with(job_id, |j| {
        j.state = "recording".into();
        j.index = 0;
        j.total = secs * fps;
        j.gif = None;
    });

    let mut rgba_frames: Vec<Vec<u8>> = Vec::new();
    let (mut vw, mut vh) = (0usize, 0usize);
    let mut next_at = Instant::now();
    let end = next_at + Duration::from_secs(secs as u64);
    let mut quiet = false;
    while Instant::now() < end {
        next_at += period;
        match crate::shot::capture_on(serial, spec.size) {
            Ok(frame) => if let Ok((w, h, rgba)) = crate::shot::render_view(spec, zones, &frame) {
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
                let frames = rgba_frames.len() as u32;
                let _ = events.send(HouseEvent::Frame {
                    port: port.to_string(),
                    png: crate::shot::encode_b64(&png),
                });
                jobs.with(job_id, |j| {
                    j.index = frames;
                });
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

    let mut final_state = "done".to_string();
    let mut final_gif: Option<String> = None;
    if !rgba_frames.is_empty() {
        let dir = captures_dir();
        let _ = std::fs::create_dir_all(&dir);
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let path = format!("{dir}/record-{port}-{stamp}.gif");
        match crate::gif::write_gif_rgba(std::path::Path::new(&path), vw, vh, delay_cs, &rgba_frames) {
            Ok(()) => final_gif = Some(path),
            Err(e) => {
                final_state = "failed".into();
                println!("[sessions] {port}: gif assembly failed — {e}");
            }
        }
    } else if quiet {
        final_state = "failed".into();
    }
    let state = final_state;
    let gif = final_gif;
    jobs.with(job_id, |j| {
        j.state = state;
        j.gif = gif;
    });
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

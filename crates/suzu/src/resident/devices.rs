//! Devices-domain state, serial sessions, and host-metrics publishing.
//!
//! Each live device owns a session thread that owns its port
//! exclusively. Stream translation, capture, recording, and admission
//! tests run on that thread because a serial port has one owner.
//! The registry controls whether the session streams (ADR-0003).
//!
//! The actor loop only routes commands and events (ADR-0004). Blocking
//! serial operations run in session threads. Whenever rows change, one
//! `Devices` event replaces the client's device collection.

use super::admission;
use super::device::{Device, DeviceAction, DeviceOrder, DeviceState, MaintenanceOrder};
use super::events::{DeviceFacts, DeviceRow, FrameFacts, ResidentEvent};
use super::jobs::{Job, Jobs};
use super::registry::DeviceRegistry;
use super::sensor::HostMetrics;
use crate::catalog::{Catalog, DisplayZones, FrameSpec, RingDialect};
use serde::Serialize;
use serialport::SerialPort;
use std::collections::BTreeMap;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::broadcast::Receiver;
use tokio::sync::{broadcast::Sender, mpsc};

/// Automatic capture interval. A session permits one capture at a time
/// and captures only while a client requests media (ADR-0004).
const FRAME_PERIOD: Duration = Duration::from_secs(2);
/// Frames older than this limit are rejected by the screenshot API.
const MAX_FRAME_AGE: Duration = Duration::from_secs(5);
/// Keepalive interval. Firmware enters its idle state after 10 seconds
/// without host data, so active sessions send at least every 5 seconds.
const KEEPALIVE_PERIOD: Duration = Duration::from_secs(5);
/// The session tick (~5 Hz): take a pending request, read current host
/// state, then wait. A new request interrupts the wait.
const TICK_PERIOD: Duration = Duration::from_millis(200);

/// Shared cache of the latest host state (ADR-0006).
/// Sensors update it and sessions send values newer than their last read.
#[derive(Default)]
pub struct HostStateCache {
    metrics: Mutex<Option<(u64, Arc<HostMetrics>)>>,
    pulse: Mutex<Option<(u64, String, u8)>>,
    next_gen: std::sync::atomic::AtomicU64,
}

impl HostStateCache {
    pub fn set_metrics(&self, metrics: Arc<HostMetrics>) {
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        *self.metrics.lock().expect("host state lock") = Some((generation, metrics));
    }

    pub fn set_pulse(&self, axis: String, value: u8) {
        let generation = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        *self.pulse.lock().expect("host state lock") = Some((generation, axis, value));
    }

    /// Return host metrics newer than `sent`, updating `sent` when consumed.
    pub fn metrics_since(&self, sent: &mut u64) -> Option<Arc<HostMetrics>> {
        let cell = self.metrics.lock().expect("host state lock");
        let (generation, metrics) = cell.as_ref()?;
        if *generation > *sent {
            *sent = *generation;
            Some(Arc::clone(metrics))
        } else {
            None
        }
    }

    /// Return the newest pulse after `sent`, updating `sent` when consumed.
    pub fn pulse_since(&self, sent: &mut u64) -> Option<(String, u8)> {
        let cell = self.pulse.lock().expect("host state lock");
        let (generation, axis, value) = cell.as_ref()?;
        if *generation > *sent {
            *sent = *generation;
            Some((axis.clone(), *value))
        } else {
            None
        }
    }
}

/// A high-priority session request. A new request replaces any pending request.
#[derive(Debug)]
pub enum SessionRequest {
    DisplayNotification { signal: String, words: Vec<String>, urgency: u8 },
    Record { job_id: String, secs: u32, fps: u32 },
    Admission,
}

/// Non-blocking single-item session mailbox (ADR-0006).
/// The newest request replaces the pending request. Host state is read
/// separately from `HostStateCache`.
#[derive(Debug, Default)]
pub struct SessionMailbox {
    request: Mutex<Option<SessionRequest>>,
    wake: Condvar,
}

impl SessionMailbox {
    pub fn submit(&self, request: SessionRequest) {
        *self.request.lock().expect("session mailbox lock") = Some(request);
        self.wake.notify_one();
    }

    pub fn take(&self) -> Option<SessionRequest> {
        self.request.lock().expect("session mailbox lock").take()
    }

    /// Wait for the next tick or until a request arrives.
    pub fn wait(&self, timeout: Duration) {
        let guard = self.request.lock().expect("session mailbox lock");
        if guard.is_some() {
            return;
        }
        let _ = self.wake.wait_timeout(guard, timeout);
    }
}

/// Ring-protocol capabilities declared by the installed faceplate (ADR-0006).
#[derive(Debug, Clone, Copy)]
pub struct RingCapabilities {
    pub qualifiers: bool,
    pub text: bool,
}

impl RingDialect {
    pub fn capabilities(&self) -> RingCapabilities {
        RingCapabilities {
            qualifiers: self.qualifiers,
            text: self.text,
        }
    }
}

/// Device read model and service state returned by the API.
/// round trip for the snapshot fact (ADR-0004).
#[derive(Debug, Clone, Serialize)]
pub struct DevicesSnapshot {
    pub devices: Vec<DeviceRow>,
    pub paused: bool,
    pub media_watched: bool,
    pub frames: Vec<FrameFacts>,
}

/// Result of changing automatic media capture.
#[derive(Debug, Clone, Serialize)]
pub struct WatchReport {
    pub changed: bool,
    #[serde(rename = "blinking")]
    pub active_captures: usize,
}

/// Result of changing global streaming: whether the pause flag changed
/// and how many tracked ports are affected.
#[derive(Debug, Clone, Serialize)]
pub struct StreamReport {
    pub changed: bool,
    pub ports: usize,
}

/// The say target's resolution ladder (ADR-0006): exact port name,
/// then unique suffix of the live enumeration — never a guess. A
/// port-shaped token that enumerates nothing is refused as such;
/// anything else is prose, not hardware.
pub enum SayTarget {
    Port(String),
    NotAPort,
    NotFound(String),
    Ambiguous(Vec<String>),
}

pub fn resolve_target_token(token: &str, ports: &[String]) -> SayTarget {
    if ports.iter().any(|p| p == token) {
        return SayTarget::Port(token.to_string());
    }
    let suffixes: Vec<String> =
        ports.iter().filter(|p| p.ends_with(token)).cloned().collect();
    match suffixes.len() {
        1 => return SayTarget::Port(suffixes[0].clone()),
        n if n > 1 => return SayTarget::Ambiguous(suffixes),
        _ => {}
    }
    let shaped = token.starts_with("/dev/")
        || (token.starts_with("COM")
            && token["COM".len()..]
                .bytes()
                .all(|b| b.is_ascii_digit()));
    if shaped {
        SayTarget::NotFound(token.to_string())
    } else {
        SayTarget::NotAPort
    }
}

/// The ring and level words a sentence may open with; everything else
/// is prose. Case-insensitive, dot-qualifiers welcome.
const SIGNAL_WORDS: &[&str] = &[
    "alert", "allclear", "completion", "discovery", "begin", "departure",
    "tended", "transition", "heartbeat", "info", "warn", "crit", "ok",
];

fn is_signal_word(word: &str) -> bool {
    let base = word.split('.').next().unwrap_or("").to_lowercase();
    SIGNAL_WORDS.contains(&base.as_str())
}

pub fn urgency_for(signal: &str) -> u8 {
    match signal.split('.').next().unwrap_or("").to_lowercase().as_str() {
        "crit" => 5,
        "alert" => 4,
        "warn" => 3,
        "heartbeat" => 0,
        _ => 2,
    }
}

/// One sentence of the say grammar:
/// `[port] [signal] [text…]` — each optional after the first.
pub struct SayParse {
    /// Some(Ok(port)) — targeted. Some(Err(reason)) — port-shaped but
    /// refused. None — broadcast.
    pub target: Option<Result<String, String>>,
    pub signal: Option<String>,
    pub text: Option<String>,
}

pub fn parse_say(sentence: &str, ports: &[String]) -> SayParse {
    let mut tokens = sentence.split_whitespace().peekable();
    let mut target = None;
    let mut signal = None;
    let mut text: Vec<&str> = Vec::new();

    if let Some(first) = tokens.peek().copied() {
        match resolve_target_token(first, ports) {
            SayTarget::Port(port) => {
                tokens.next();
                target = Some(Ok(port));
                if let Some(second) = tokens.next() {
                    if is_signal_word(second) {
                        signal = Some(second.to_lowercase());
                    } else {
                        text.push(second);
                    }
                }
            }
            SayTarget::NotFound(r) => {
                target = Some(Err(format!("{r}: no such port on this machine")));
            }
            SayTarget::Ambiguous(list) => {
                target = Some(Err(format!(
                    "{first} is ambiguous — {}; say which",
                    list.join(", ")
                )));
            }
            SayTarget::NotAPort => {}
        }
    }
    if target.is_none()
        && let Some(first) = tokens.next() {
            if is_signal_word(first) {
                signal = Some(first.to_lowercase());
            } else {
                text.push(first);
            }
        }
    text.extend(tokens);
    SayParse {
        target,
        signal,
        text: (!text.is_empty()).then(|| text.join(" ")),
    }
}

pub enum DevicesCmd {
    Track(DeviceFacts),
    Gone { port: String },
    /// The read model, taken by the owning domain. Answers at once —
    /// the loop routes, it never waits.
    Snapshot { reply: mpsc::Sender<DevicesSnapshot> },
    /// The newest frame for a port, under the freshness bound. An
    /// error when no recent frame is available.
    LatestFrame { port: String, reply: mpsc::Sender<anyhow::Result<Vec<u8>>> },
    /// Save the newest frame into the captures folder; replies the path.
    CaptureSave { port: String, reply: mpsc::Sender<anyhow::Result<String>> },
    /// Stop streaming and release ports, or reopen them on resume.
    /// This state resets when the process restarts.
    Pause { reply: mpsc::Sender<StreamReport> },
    Resume { reply: mpsc::Sender<StreamReport> },
    /// Enable or disable automatic captures for media clients.
    WatchMedia { on: bool, reply: Option<mpsc::Sender<WatchReport>> },
    /// Request a saved screenshot from the owning session. Reports whether the
    /// session took the job; the verdict travels as Job facts.
    RecordStart { port: String, job_id: String, secs: u32, fps: u32, reply: mpsc::Sender<anyhow::Result<()>> },
    /// Re-run admission tests through the owning session.
    AdmissionRetry { port: String, reply: mpsc::Sender<anyhow::Result<()>> },
    /// Send one targeted display message (ADR-0006), adjusted to the
    /// faceplate's declared ring capabilities.
    Say {
        port: String,
        signal: String,
        text: Option<String>,
        reply: mpsc::Sender<anyhow::Result<()>>,
    },
    /// Shared device-action vocabulary for every client. The device
    /// aggregate decides whether the action is legal and returns an
    /// order; this actor only enacts it.
    Act {
        port: String,
        action: DeviceAction,
        faceplate: Option<String>,
        reply: mpsc::Sender<anyhow::Result<()>>,
    },
    /// Return maintenance results and re-identified device facts.
    MaintenanceFinished {
        port: String,
        device_id: String,
        ok: bool,
        fresh: Option<DeviceFacts>,
    },
}

/// Ports currently reserved by maintenance. The watcher skips them so
/// probe DTR/RTS changes cannot reset a device during writes (ADR-0002).
pub(crate) fn reserved_ports() -> &'static std::sync::Mutex<std::collections::HashSet<String>> {
    use std::sync::OnceLock;
    static RESERVED: OnceLock<std::sync::Mutex<std::collections::HashSet<String>>> =
        OnceLock::new();
    RESERVED.get_or_init(|| std::sync::Mutex::new(std::collections::HashSet::new()))
}

pub struct Devices {
    events: Sender<ResidentEvent>,
    command_tx: mpsc::Sender<DevicesCmd>,
    registry: Arc<std::sync::RwLock<DeviceRegistry>>,
    catalog: Arc<Catalog>,
    devices: BTreeMap<String, Device>,
    sessions: BTreeMap<String, SessionHandle>,
    jobs: Arc<Jobs>,
    /// Newest captured frame and timestamp per port.
    frames: BTreeMap<String, (Instant, String)>,
    /// port → (kind, faceplate) while maintenance runs (ADR-0005).
    in_maintenance: BTreeMap<String, (String, Option<String>)>,
    /// Automatic faceplate-update attempts per port since the last successful attach.
    auto_faceplate_updates: BTreeMap<String, u8>,
    paused: bool,
    /// Whether a media client currently requests automatic captures.
    media_watched: Arc<AtomicBool>,
    /// Latest host metrics and pulses read by sessions (ADR-0006).
    host_state: Arc<HostStateCache>,
    rows_dirty: bool,
}

struct SessionHandle {
    mailbox: Arc<SessionMailbox>,
    close: Arc<AtomicBool>,
    streaming: Arc<AtomicBool>,
    supports_capture: bool,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Devices {
    pub fn new(
        events: Sender<ResidentEvent>,
        command_tx: mpsc::Sender<DevicesCmd>,
        catalog: Arc<Catalog>,
        registry: Arc<std::sync::RwLock<DeviceRegistry>>,
        jobs: Arc<Jobs>,
        host_state: Arc<HostStateCache>,
    ) -> Self {
        Self {
            events,
            command_tx,
            registry,
            catalog,
            host_state,
            devices: BTreeMap::new(),
            sessions: BTreeMap::new(),
            jobs,
            frames: BTreeMap::new(),
            in_maintenance: BTreeMap::new(),
            auto_faceplate_updates: BTreeMap::new(),
            paused: false,
            media_watched: Arc::new(AtomicBool::new(false)),
            rows_dirty: false,
        }
    }

    pub async fn run(mut self, mut rx: mpsc::Receiver<DevicesCmd>, mut bus: Receiver<ResidentEvent>) {
        loop {
            tokio::select! {
                cmd = rx.recv() => {
                    let Some(cmd) = cmd else { break };
                    match cmd {
                        DevicesCmd::Track(facts) => self.track(facts),
                        DevicesCmd::Gone { port } => self.gone(&port),
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
                        DevicesCmd::RecordStart { port, job_id, secs, fps, reply } => {
                            let res = self.record_start(&port, &job_id, secs, fps);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::AdmissionRetry { port, reply } => {
                            let res = self.admission_retry(&port);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::Say { port, signal, text, reply } => {
                            let res = self.say_to(&port, &signal, text.as_deref());
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::Act { port, action, faceplate, reply } => {
                            let res = self.act(&port, action, faceplate);
                            let _ = reply.send(res).await;
                        }
                        DevicesCmd::MaintenanceFinished { port, device_id, ok, fresh } => {
                            self.maintenance_finish(&port, &device_id, ok, fresh).await;
                        }
                    }
                }
                ev = bus.recv() => {
                    match ev {
                        Ok(ResidentEvent::StreamAttached { port, .. }) => {
                            self.auto_faceplate_updates.remove(&port);
                            if let Some(session) = self.sessions.get(&port) {
                                session.streaming.store(true, Ordering::Relaxed);
                                self.rows_dirty = true;
                            }
                        }
                        Ok(ResidentEvent::StreamDetached { port, .. }) => {
                            if let Some(session) = self.sessions.get(&port) {
                                session.streaming.store(false, Ordering::Relaxed);
                                self.rows_dirty = true;
                            }
                        }
                        // Cache session frames for capture requests and snapshots.
                        Ok(ResidentEvent::Frame { port, png }) => {
                            self.frames.insert(port, (Instant::now(), png));
                        }
                        // Broadcast a display notification to every active session.
                        Ok(ResidentEvent::DisplayNotificationReady { signal, label, urgency }) => {
                            if !self.paused {
                                self.send_notification(&signal, &label, urgency);
                            }
                        }
                        // Automatically update a faceplate whose reported
                        // version is older than the catalog declaration.
                        Ok(ResidentEvent::AdmissionReport {
                            port,
                            passed: false,
                            steps,
                            ..
                        }) => {
                            // exactly one failure, and it the stale-declared
                            // verdict ("older than", never "not declared")
                            let failed: Vec<_> =
                                steps.iter().filter(|s| !s.ok).collect();
                            let stale_declared = failed.len() == 1
                                && failed[0].name == "faceplate-version"
                                && failed[0].detail.contains("older than the declared");
                            let attempts = self
                                .auto_faceplate_updates
                                .entry(port.clone())
                                .and_modify(|n| *n += 1)
                                .or_insert(1);
                            if stale_declared
                                && !self.in_maintenance.contains_key(&port)
                                && *attempts <= 2
                                && let Some(faceplate_id) = self.devices.get(&port)
                                    .and_then(|d| {
                                        self.catalog
                                            .installed_faceplate(
                                                d.facts.class.as_deref().unwrap_or_default(),
                                                d.facts.faceplate.as_deref().unwrap_or_default(),
                                                d.facts.mount.as_deref(),
                                            )
                                            .map(|info| info.id.clone())
                                    })
                            {
                                println!(
                                    "[devices] {port}: faceplate {faceplate_id} is outdated; starting automatic update"
                                );
                                let _ = self.act(
                                    &port,
                                    DeviceAction::Update,
                                    Some(faceplate_id),
                                );
                            }
                        }
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                        Err(_) => break,
                    }
                }
            }
            // One event replaces
            // every client's slice whenever the rows changed.
            if self.rows_dirty {
                self.rows_dirty = false;
                let rows = self.snapshot();
                let _ = self.events.send(ResidentEvent::Devices { rows });
            }
        }
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
    fn spawn_session(&mut self, facts: &DeviceFacts, identity_store: Option<String>) {
        // Only suzu/1 devices can stream. Other recognized devices remain
        // tracked as New until provisioning completes.
        let suzu = facts.proto.as_deref() == Some("suzu/1");
        if !suzu {
            self.rows_dirty = true;
            return;
        }
        let mailbox = Arc::new(SessionMailbox::default());
        let thread_mailbox = Arc::clone(&mailbox);
        let (spec, zones) = self.frame_law_of(facts);
        let supports_capture = suzu && spec.is_some();
        // Use the faceplate's declared ring-protocol capabilities.
        let ring_capabilities = facts
            .faceplate
            .as_deref()
            .zip(facts.class.as_deref())
            .and_then(|(fp, class)| self.catalog.faceplate(class, fp))
            .map(|f| f.rings.capabilities())
            .unwrap_or(RingCapabilities { qualifiers: true, text: true });
        let streaming = Arc::new(AtomicBool::new(false));
        let close = Arc::new(AtomicBool::new(false));
        let port = facts.port.clone();
        let events = self.events.clone();
        let jobs = Arc::clone(&self.jobs);
        let media_watched = Arc::clone(&self.media_watched);
        let host_state = Arc::clone(&self.host_state);
        let device_id = facts.device_id.clone();
        let class = facts.class.clone();
        // Compare the reported faceplate version with the catalog version.
        // Undeclared faceplates are treated as outdated. Classes with no
        // faceplates or no declared version skip this admission check.
        let faceplate_versions = match (&facts.faceplate, &facts.class) {
            (Some(fp), Some(class)) => {
                let declared =
                    self.catalog.installed_faceplate(class, fp, facts.mount.as_deref());
                if declared.is_none()
                    && !self.catalog.faceplates_for_class(class).is_empty()
                {
                    Some((fp.clone(), None)) // reported but undeclared
                } else {
                    declared
                        .and_then(|f| f.version.clone())
                        .map(|v| (facts.version.clone().unwrap_or_default(), Some(v)))
                }
            }
            _ => None,
        };
        let streaming2 = Arc::clone(&streaming);
        let close2 = Arc::clone(&close);
        let join = std::thread::Builder::new()
            .name(format!("session:{port}"))
            .spawn(move || {
                session_thread(
                    port, thread_mailbox, host_state.clone(), close2, streaming2,
                    suzu, spec, zones, events, jobs, media_watched, ring_capabilities,
                    device_id, class, identity_store, faceplate_versions,
                )
            })
            .ok();
        self.sessions.insert(
            facts.port.clone(),
            SessionHandle { mailbox, close, streaming, supports_capture, join },
        );
        self.rows_dirty = true;
    }

    fn close_session(&mut self, port: &str) -> Option<std::thread::JoinHandle<()>> {
        let mut handle = self.sessions.remove(port)?;
        handle.close.store(true, Ordering::Relaxed);
        handle.streaming.store(false, Ordering::Relaxed);
        self.frames.remove(port);
        handle.join.take()
    }

    /// Broadcast a display notification to every streaming session.
    fn send_notification(&mut self, signal: &str, label: &str, urgency: u8) {
        let words: Vec<String> = label.split_whitespace().map(|s| s.to_string()).collect();
        for session in self.sessions.values() {
            if session.streaming.load(Ordering::Relaxed) {
                let mailbox = &session.mailbox;
                mailbox.submit(SessionRequest::DisplayNotification {
                    signal: signal.to_string(),
                    words: words.clone(),
                    urgency,
                });
            }
        }
    }

    /// Enable or disable automatic media capture. The operation is idempotent.
    fn watch_media(&mut self, on: bool) -> WatchReport {
        let changed = self.media_watched.swap(on, Ordering::Relaxed) != on;
        if changed {
            let _ = self.events.send(ResidentEvent::MediaWatched { watched: on });
        }
        WatchReport {
            changed,
            active_captures: self
                .sessions
                .values()
                .filter(|s| s.supports_capture && s.streaming.load(Ordering::Relaxed))
                .count(),
        }
    }

    /// Pause all sessions and release serial ports while retaining device state.
    fn pause_stream(&mut self) -> StreamReport {
        if self.paused {
            return StreamReport { changed: false, ports: self.devices.len() };
        }
        self.paused = true;
        let ports: Vec<String> = self.devices.keys().cloned().collect();
        for port in &ports {
            self.close_session(port);
        }
        self.rows_dirty = true;
        let _ = self.events.send(ResidentEvent::Paused { paused: true });
        println!(
            "[devices] stream paused — {} serial port(s) released (`suzu resume` to restart)",
            self.devices.len()
        );
        StreamReport { changed: true, ports: self.devices.len() }
    }

    /// Resume by reopening sessions and repeating admission tests.
    async fn resume_stream(&mut self) -> StreamReport {
        if !self.paused {
            return StreamReport { changed: false, ports: self.devices.len() };
        }
        self.paused = false;
        let ports: Vec<String> = self.devices.keys().cloned().collect();
        println!("[devices] stream resumed — re-opening {} session(s)", ports.len());
        for port in ports {
            let facts = self.devices[&port].facts.clone();
            self.spawn_session(&facts, None);
            let _ = self.events.send(ResidentEvent::DeviceTracked {
                port,
                device_id: facts.device_id.clone(),
                class: facts.class.clone(),
                state: format!("{:?}", DeviceState::Accepted),
            });
        }
        let _ = self.events.send(ResidentEvent::Paused { paused: false });
        StreamReport { changed: true, ports: self.devices.len() }
    }

    fn track(&mut self, mut facts: DeviceFacts) {
        let state = DeviceState::Accepted;
        // Assign an ID immediately when a recognized device has none.
        // The session persists it to the device below.
        let identity_assigned = facts.class.is_some() && facts.device_id.is_none();
        if identity_assigned {
            facts.device_id = Some(crate::prepare::mint_v7());
            println!(
                "[devices] {} assigned identity {}",
                facts.port,
                facts.device_id.as_deref().unwrap_or("?")
            );
        }
        match self.devices.get(&facts.port) {
            Some(existing) if existing.device_id() == facts.device_id.as_deref() => {
                let _ = self.events.send(ResidentEvent::DeviceReconnected {
                    port: facts.port.clone(),
                    device_id: facts.device_id.clone().unwrap_or_default(),
                });
            }
            _ => {
                let _ = self.events.send(ResidentEvent::DeviceTracked {
                    port: facts.port.clone(),
                    device_id: facts.device_id.clone(),
                    class: facts.class.clone(),
                    state: format!("{state:?}"),
                });
            }
        }

        self.devices
            .insert(facts.port.clone(), Device::new(facts.clone(), now()));
        self.rows_dirty = true;
        // Do not start a session while global streaming is paused.
        if !self.paused {
            let identity_store = if identity_assigned { facts.device_id.clone() } else { None };
            self.spawn_session(&facts, identity_store);
        }
    }

    fn gone(&mut self, port: &str) {
        if let Some(device) = self.devices.remove(port) {
            self.close_session(port);
            self.rows_dirty = true;
            let _ = self.events.send(ResidentEvent::DeviceReleased {
                port: port.to_string(),
                device_id: device.device_id().map(|s| s.to_string()),
            });
            // If the port remains enumerated after a session exits, retry it
            // after a short delay. Unplugged devices are not retried.
            let facts = device.facts.clone();
            let command_tx = self.command_tx.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(3)).await;
                let still_present = crate::enumerate().iter().any(|e| e.name == facts.port);
                if still_present {
                    println!("[devices] {} is still connected — retrying the session", facts.port);
                    let _ = command_tx.send(DevicesCmd::Track(facts)).await;
                }
            });
        }
    }

    /// Return the newest cached frame when it satisfies the freshness limit.
    fn latest_frame(&self, port: &str) -> anyhow::Result<Vec<u8>> {
        let Some((at, png_b64)) = self.frames.get(port) else {
            anyhow::bail!("{port}: no captured frame is available");
        };
        let age = at.elapsed();
        if age > MAX_FRAME_AGE {
            anyhow::bail!(
                "{port}: the latest frame is {}s old; the device may be unreachable",
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

    /// Start a recording job on the owning session.
    fn record_start(&mut self, port: &str, job_id: &str, secs: u32, fps: u32) -> anyhow::Result<()> {
        let secs = secs.clamp(1, 60);
        let fps = fps.clamp(1, 5);
        if let Err(e) = self.send_to_session(port, SessionRequest::Record { job_id: job_id.to_string(), secs, fps }) {
            self.jobs.with(job_id, |j: &mut Job| {
                j.state = "failed".into();
                j.label = format!("{e:#}");
            });
            return Err(e);
        }
        Ok(())
    }

    fn admission_retry(&self, port: &str) -> anyhow::Result<()> {
        self.send_to_session(port, SessionRequest::Admission)
    }

    /// Send a targeted display message without the broadcast rate limit.
    fn say_to(&mut self, port: &str, signal: &str, text: Option<&str>) -> anyhow::Result<()> {
        let session = self
            .sessions
            .get(port)
            .ok_or_else(|| anyhow::anyhow!("{port}: no live session"))?;
        if !session.streaming.load(Ordering::Relaxed) {
            anyhow::bail!("{port}: is not currently streaming");
        }
        session.mailbox.submit(SessionRequest::DisplayNotification {
            signal: signal.to_string(),
            words: text
                .unwrap_or("")
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
            urgency: urgency_for(signal),
        });
        Ok(())
    }

    /// Validate a device action, then execute the resulting order.
    /// Lifecycle and faceplate rules live on `Device`;
    /// this actor owns only infrastructure and event publication.
    fn act(
        &mut self,
        port: &str,
        action: DeviceAction,
        faceplate: Option<String>,
    ) -> anyhow::Result<()> {
        let order = {
            let device = self
                .devices
                .get(port)
                .ok_or_else(|| anyhow::anyhow!("{port}: no tracked device"))?;
            let in_maintenance = self.in_maintenance.contains_key(port);
            match action {
                DeviceAction::Pause => {
                    let mut registry = self
                        .registry
                        .write()
                        .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
                    device.pause(&mut registry)?
                }
                DeviceAction::Resume => {
                    let mut registry = self
                        .registry
                        .write()
                        .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
                    device.resume(&mut registry)?
                }
                DeviceAction::Identify => {
                    let registry = self
                        .registry
                        .read()
                        .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
                    let registered_device = device.device_id().and_then(|id| registry.registered_device(id));
                    device.identify(registered_device)?
                }
                DeviceAction::Install => {
                    let registry = self
                        .registry
                        .read()
                        .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
                    let registered_device = device.device_id().and_then(|id| registry.registered_device(id));
                    device.install(&self.catalog, faceplate, registered_device, in_maintenance)?
                }
                DeviceAction::Update => {
                    let registry = self
                        .registry
                        .read()
                        .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
                    let registered_device = device.device_id().and_then(|id| registry.registered_device(id));
                    device.update(&self.catalog, faceplate, registered_device, in_maintenance)?
                }
                DeviceAction::FactoryReset => {
                    let registry = self
                        .registry
                        .read()
                        .map_err(|_| anyhow::anyhow!("registry lock poisoned"))?;
                    let registered_device = device.device_id().and_then(|id| registry.registered_device(id));
                    device.factory_reset(&self.catalog, registered_device, in_maintenance)?
                }
            }
        };
        self.enact(port, order)
    }

    fn enact(&mut self, port: &str, order: DeviceOrder) -> anyhow::Result<()> {
        match order {
            DeviceOrder::Pause { device_id } => {
                if let Some(session) = self.sessions.get(port) {
                    session.streaming.store(false, Ordering::Relaxed);
                }
                self.rows_dirty = true;
                let _ = self.events.send(ResidentEvent::StreamDetached {
                    device_id,
                    port: port.to_string(),
                    reason: "paused by user request".into(),
                });
            }
            DeviceOrder::Resume { device_id } => {
                if let Some(session) = self.sessions.get(port) {
                    session.streaming.store(true, Ordering::Relaxed);
                }
                self.rows_dirty = true;
                let _ = self.events.send(ResidentEvent::StreamAttached {
                    device_id,
                    port: port.to_string(),
                });
            }
            DeviceOrder::Identify => {
                self.say_to(port, "info", Some(&format!("Hello from {port}!")))?;
            }
            DeviceOrder::Maintenance(order) => self.maintenance_begin(port, order)?,
        }
        Ok(())
    }

    fn send_to_session(&self, port: &str, request: SessionRequest) -> anyhow::Result<()> {
        let session = self
            .sessions
            .get(port)
            .ok_or_else(|| anyhow::anyhow!("{port}: no live session — is the stream paused?"))?;
        session.mailbox.submit(request);
        Ok(())
    }

    /// Stop the device session, run maintenance with exclusive serial-port
    /// access, then restart the session and admission tests. The command
    /// acknowledges after startup; progress is published as events.
    fn maintenance_begin(&mut self, port: &str, order: MaintenanceOrder) -> anyhow::Result<()> {
        // The order contains the validated caller intent. Current device
        // facts remain authoritative.
        let MaintenanceOrder { kind, faceplate, .. } = order;
        let kind = kind.as_str();
        // Unknown firmware uses provisioning; suzu/1 firmware uses reinstall.
        let speaks_suzu = self
            .devices
            .get(port)
            .and_then(|d| d.facts.proto.as_deref())
            == Some("suzu/1");
        let kind = if kind == "install" && !speaks_suzu {
            "provision"
        } else {
            kind
        };
        if kind != "install" && kind != "provision" && kind != "soft" && kind != "factory" {
            anyhow::bail!(
                "unknown maintenance kind {kind:?} - install | provision | soft | factory"
            );
        }
        if self.in_maintenance.contains_key(port) {
            anyhow::bail!("{port}: a maintenance procedure is already running");
        }
        let Some(device) = self.devices.get(port) else {
            anyhow::bail!("{port}: no tracked device");
        };
        let Some(device_id) = device.device_id().map(|s| s.to_string()) else {
            anyhow::bail!("{port}: no device_id was assigned during identification");
        };
        let class = device.facts.class.clone();
        // Validate the faceplate ID against the class catalog.
        if let Some(faceplate_id) = &faceplate {
            let declared = class
                .as_deref()
                .map(|c| self.catalog.faceplates_for_class(c))
                .unwrap_or_default();
            if !declared.iter().any(|f| &f.id == faceplate_id) {
                let vocab = declared
                    .iter()
                    .map(|f| format!("{:?}", f.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "unknown faceplate {faceplate_id:?} — this class declares: {}",
                    if vocab.is_empty() { "none".to_string() } else { vocab }
                );
            }
        }
        let vid = device.facts.vid;
        let pid = device.facts.pid;

        // An update without an explicit selection keeps the currently
        // reported faceplate when it is still declared.
        let faceplate = match faceplate {
            Some(f) => Some(f),
            None => {
                // Resolve the reported faceplate and mount to its install ID.
                self.devices.get(port).and_then(|d| {
                    self.catalog
                        .installed_faceplate(
                            d.facts.class.as_deref().unwrap_or_default(),
                            d.facts.faceplate.as_deref().unwrap_or_default(),
                            d.facts.mount.as_deref(),
                        )
                        .map(|info| info.id.clone())
                })
            }
        };

        // Detach streaming and release the port for maintenance.
        let _ = self.events.send(ResidentEvent::StreamDetached {
            device_id: device_id.clone(),
            port: port.to_string(),
            reason: format!("maintenance:{kind}"),
        });
        // Join the stopped session thread before maintenance opens the port.
        let joining = self.close_session(port);
        self.in_maintenance
            .insert(port.to_string(), (kind.to_string(), faceplate.clone()));
        reserved_ports().lock().unwrap().insert(port.to_string());
        self.rows_dirty = true;

        let _ = self.events.send(ResidentEvent::MaintenanceStarted {
            device_id: device_id.clone(),
            port: port.to_string(),
            kind: kind.to_string(),
        });

        // Maintenance runs on a blocking worker; completion returns through
        // the devices command channel.
        let events = self.events.clone();
        let catalog = Arc::clone(&self.catalog);
        let catalog_fresh = Arc::clone(&self.catalog);
        let device_id2 = device_id.clone();
        let port2 = port.to_string();
        let port3 = port2.clone();
        let kind2 = kind.to_string();
        let faceplate2 = faceplate.clone();
        let command_tx = self.command_tx.clone();
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
            .unwrap_or_else(|e| Err(anyhow::anyhow!("maintenance task panicked: {e}")));
            let ok = outcome.is_ok();
            if let Err(e) = &outcome {
                println!("[maintenance] {port3}: failed — {e:#}");
            }
            // Retry identification while the device boots and compiles source.
            let port4 = port3.clone();
            let fresh = tokio::task::spawn_blocking(move || {
                let mut out = None;
                for _ in 0..6 {
                    match super::watcher::identify_facts(&catalog_fresh, &port4, vid, pid) {
                        Ok(facts) if facts.proto.as_deref() == Some("suzu/1") => {
                            out = Some(facts);
                            break;
                        }
                        other => out = other.ok(),
                    }
                    std::thread::sleep(std::time::Duration::from_millis(2500));
                }
                out
            })
            .await
            .unwrap_or(None);
            let _ = command_tx
                .send(DevicesCmd::MaintenanceFinished { port: port3, device_id, ok, fresh })
                .await;
        });
        Ok(())
    }

    /// Record maintenance completion, update device facts, restart the
    /// session, and run admission tests.
    async fn maintenance_finish(
        &mut self,
        port: &str,
        device_id: &str,
        ok: bool,
        fresh: Option<DeviceFacts>,
    ) {
        let _ = self.events.send(ResidentEvent::MaintenanceCompleted {
            device_id: device_id.to_string(),
            kind: self
                .in_maintenance
                .get(port)
                .map(|(kind, _)| kind.clone())
                .unwrap_or_else(|| "unknown".into()),
            ok,
        });
        self.in_maintenance.remove(port);
        reserved_ports().lock().unwrap().remove(port);
        if let Some(session) = self.sessions.get(port) {
            session.streaming.store(false, Ordering::Relaxed);
        }
        self.rows_dirty = true;
        match fresh {
            Some(new_facts) if self.devices.contains_key(port) && !self.paused => {
                self.session_respawn(port, new_facts);
            }
            Some(_) => {} // gone or paused meanwhile: the respawn waits
            None => {
                if self.devices.contains_key(port) {
                    println!("[maintenance] {port}: the port went quiet after maintenance — replug to re-admit");
                }
            }
        }
    }

    /// Update device facts after maintenance and restart its session.
    fn session_respawn(&mut self, port: &str, facts: DeviceFacts) {
        let Some(d) = self.devices.get_mut(port) else {
            return; // The device disconnected during maintenance.
        };
        println!(
            "[maintenance] {port}: re-identified as {}/{} — respawning",
            facts.family.as_deref().unwrap_or("?"),
            facts.variant.as_deref().unwrap_or("?")
        );
        d.facts = facts.clone();
        if !self.paused {
            self.spawn_session(&facts, None);
        }
    }

    pub fn snapshot(&self) -> Vec<DeviceRow> {
        let registry = self.registry.read().ok();
        self.devices
            .values()
            .map(|d| {
                let registered_device = registry.as_ref().and_then(|r| {
                    d.device_id().and_then(|id| r.registered_device(id))
                });
                let session = self.sessions.get(&d.facts.port);
                DeviceRow {
                port: d.facts.port.clone(),
                class: d.facts.class.clone(),
                family: d.facts.family.clone(),
                variant: d.facts.variant.clone(),
                version: d.facts.version.clone(),
                proto: d.facts.proto.clone(),
                device_id: d.facts.device_id.clone(),
                state: d.state.clone(),
                actions: d.available_actions(
                    registered_device,
                    self.in_maintenance.contains_key(&d.facts.port),
                ),
                faceplate: d.facts.faceplate.clone(),
                mount: d.facts.mount.clone(),
                lifecycle: registered_device
                    .map(|i| i.lifecycle)
                    .map(|l| format!("{l:?}").to_lowercase()),
                streaming: session
                    .is_some_and(|s| s.streaming.load(Ordering::Relaxed)),
                last_data_s: None,
            }
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
    crate::paths::captures_dir().to_string_lossy().into_owned()
}

// ── the session — one thread per device, one master of the port ────

#[allow(clippy::too_many_arguments)]
fn session_thread(
    port: String,
    mailbox: Arc<SessionMailbox>,
    host_state: Arc<HostStateCache>,
    close: Arc<AtomicBool>,
    streaming: Arc<AtomicBool>,
    suzu: bool,
    spec: Option<FrameSpec>,
    zones: DisplayZones,
    events: Sender<ResidentEvent>,
    jobs: Arc<Jobs>,
    media_watched: Arc<AtomicBool>,
    ring_capabilities: RingCapabilities,
    device_id: Option<String>,
    class: Option<String>,
    identity_store: Option<String>,
    faceplate_versions: Option<(String, Option<String>)>,
) {
    // A previous session thread may still be exiting because capture can take
    // up to eight seconds. Retry before declaring the port unreachable.
    let mut serial = None;
    for attempt in 0..12 {
        if close.load(Ordering::Relaxed) {
            return; // The session was closed before the port opened.
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
        "[sessions] {port}: session started ({})",
        if suzu { "suzu/1" } else { "unknown protocol" }
    );

    // Run admission before enabling streaming.
    let admission_faceplate_versions = faceplate_versions
        .as_ref()
        .map(|(installed, declared)| (installed.as_str(), declared.as_deref()));
    if suzu {
        let report = admission::run(
            &mut serial,
            class.as_deref(),
            spec.as_ref(),
            &zones,
            admission_faceplate_versions,
        );
        let _ = events.send(ResidentEvent::AdmissionReport {
            device_id: device_id.clone().unwrap_or_default(),
            port: port.clone(),
            passed: report.passed,
            steps: report.steps,
        });
    }

    // Persist a newly assigned identity as soon as the session opens.
    // Identity persists even if admission tests fail.
    if let Some(id) = identity_store {
        let _ = write_line(&mut serial, &format!("J,{{\"device_id\":\"{id}\"}}"));
        std::thread::sleep(Duration::from_millis(300));
    }

    // Catch panics so the serial port is released deterministically.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        session_loop(
            &mut serial, &mailbox, &host_state, &close, &streaming, suzu, &port,
            spec, zones, &events, &jobs, &media_watched, ring_capabilities, device_id,
            class, faceplate_versions.clone(),
        );
    }));
    if outcome.is_err() {
        println!("[sessions] {port}: session panicked — port released");
    }
    println!("[sessions] {port}: serial port released");
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

/// Send one host-metrics update with one retry. False
/// means the wire refused twice — the session is over.
fn deliver_metrics(
    serial: &mut Box<dyn SerialPort>,
    port: &str,
    metrics: &Arc<HostMetrics>,
    named: &mut Option<String>,
) -> bool {
    let frames = encode_metric_frames(metrics, named);
    for frame in frames {
        if write_line_twice(serial, port, &frame).is_err() {
            return false;
        }
    }
    true
}

/// Capture, render, and publish one display frame.
fn capture_frame(
    serial: &mut Box<dyn SerialPort>,
    port: &str,
    spec: &FrameSpec,
    zones: &[(usize, usize, [u8; 3])],
    events: &Sender<ResidentEvent>,
) {
    let png = crate::shot::capture_on(serial, spec.size)
        .and_then(|frame| crate::shot::render_png_bytes(spec, zones, &frame));
    if let Err(e) = &png {
        println!("[sessions] {port}: blink failed — {e:#}");
    }
    if let Ok(png) = png {
        let _ = events.send(ResidentEvent::Frame {
            port: port.to_string(),
            png: crate::shot::encode_b64(&png),
        });
    }
}

#[allow(clippy::too_many_arguments)]
fn session_loop(
    serial: &mut Box<dyn SerialPort>,
    mailbox: &SessionMailbox,
    host_state: &HostStateCache,
    close: &Arc<AtomicBool>,
    streaming: &Arc<AtomicBool>,
    suzu: bool,
    port: &str,
    spec: Option<FrameSpec>,
    zones: DisplayZones,
    events: &Sender<ResidentEvent>,
    jobs: &Jobs,
    media_watched: &Arc<AtomicBool>,
    ring_capabilities: RingCapabilities,
    device_id: Option<String>,
    class: Option<String>,
    faceplate_versions: Option<(String, Option<String>)>,
) {
    let mut named: Option<String> = None;
    let mut seq: u8 = 0;
    let mut sent_metrics_generation: u64 = 0;
    let mut sent_pulse_gen: u64 = 0;
    let mut next_frame = Instant::now() + FRAME_PERIOD;
    let mut last_keepalive = Instant::now();

    loop {
        if close.load(Ordering::Relaxed) {
            break;
        }
        // Process pending commands before sending new host state.
        while let Some(request) = mailbox.take() {
            match request {
                SessionRequest::DisplayNotification { signal, words, urgency } => {
                    if !suzu || !streaming.load(Ordering::Relaxed) {
                        continue;
                    }
                    // Remove qualifiers or text that the faceplate does not
                    // support (ADR-0006).
                    seq = seq.wrapping_add(1);
                    let sig = if ring_capabilities.qualifiers {
                        signal.clone()
                    } else {
                        signal.split('.').next().unwrap_or(&signal).to_string()
                    };
                    let supported_words = if ring_capabilities.text {
                        words
                    } else {
                        Vec::new()
                    };
                    let mut frame =
                        format!("R,{sig},{urgency},0,1,{seq},{}", supported_words.join(","));
                    frame = with_checksum(&frame);
                    if write_line_twice(serial, port, &frame).is_err() {
                        return;
                    }
                }
                SessionRequest::Record { job_id, secs, fps } => {
                    record_job(serial, port, &job_id, secs, fps, spec.as_ref(), &zones, jobs, events);
                }
                SessionRequest::Admission => {
                    if suzu {
                        let versions = faceplate_versions
                            .as_ref()
                            .map(|(installed, declared)| {
                                (installed.as_str(), declared.as_deref())
                            });
                        let report = admission::run(
                            serial,
                            class.as_deref(),
                            spec.as_ref(),
                            &zones,
                            versions,
                        );
                        let _ = events.send(ResidentEvent::AdmissionReport {
                            device_id: device_id.clone().unwrap_or_default(),
                            port: port.to_string(),
                            passed: report.passed,
                            steps: report.steps,
                        });
                    }
                }
            }
        }
        if streaming.load(Ordering::Relaxed) {
            if let Some(metrics) = host_state.metrics_since(&mut sent_metrics_generation)
                && !deliver_metrics(serial, port, &metrics, &mut named)
            {
                return;
            }
            if let Some((axis, value)) = host_state.pulse_since(&mut sent_pulse_gen) {
                let frame = format!("A,{axis},{value}");
                if write_line_twice(serial, port, &frame).is_err() {
                    return;
                }
            }
        }
        let now = Instant::now();
        // Automatic capture requires streaming and a media subscriber.
        // Recording publishes frames from its own handler.
        if now >= next_frame {
            next_frame = now + FRAME_PERIOD;
            if suzu
                && streaming.load(Ordering::Relaxed)
                && media_watched.load(Ordering::Relaxed)
                && let Some(spec) = &spec
            {
                capture_frame(serial, port, spec, &zones, events);
            }
        }
        // Send keepalives only while streaming.
        if now.duration_since(last_keepalive) >= KEEPALIVE_PERIOD {
            last_keepalive = now;
            if streaming.load(Ordering::Relaxed) {
                let keepalive = if suzu { "K" } else { "R" };
                let _ = write_line(serial, keepalive);
            }
        }
        // Wait for the next polling interval or an incoming command.
        mailbox.wait(TICK_PERIOD);
    }
}

/// Capture frames from one session, publish previews, and assemble a GIF.
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
    events: &Sender<ResidentEvent>,
) {
    let Some(spec) = spec else {
        jobs.with(job_id, |j| {
            j.state = "failed".into();
            j.label = "the class declares no frame format".into();
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
                let png = {
                    let rgb: Vec<[u8; 3]> =
                        rgba.chunks_exact(4).map(|p| [p[0], p[1], p[2]]).collect();
                    crate::shot::png_bytes(w, h, &rgb).unwrap_or_default()
                };
                vw = w;
                vh = h;
                rgba_frames.push(rgba);
                let frames = rgba_frames.len() as u32;
                let _ = events.send(ResidentEvent::Frame {
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
            next_at = now; // wire-bound: skip the missed capture interval, keep going
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

fn encode_metric_frames(metrics: &HostMetrics, named: &mut Option<String>) -> Vec<String> {
    let mut frames = Vec::new();
    if named.as_deref() != Some(metrics.name.as_str()) {
        frames.push(format!("J,{{\"name\":\"{}\"}}", metrics.name.replace('"', "'")));
        *named = Some(metrics.name.clone());
    }
    frames.push(format!(
        "G,report,{},{},{}",
        metrics.cpu,
        metrics.mem,
        metrics.gpu.unwrap_or(255)
    ));
    frames
}

fn open_serial(port: &str) -> anyhow::Result<Box<dyn SerialPort>> {
    let mut p = serialport::new(port, 115_200)
        .timeout(Duration::from_millis(200))
        .open()
        .map_err(|e| anyhow::anyhow!("{port}: {e}"))?;
    // CircuitPython requires DTR before its CDC console accepts host metrics.
    let _ = p.write_data_terminal_ready(true);
    // Opening the ESP serial port triggers reset; wait for boot completion.
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

#[cfg(test)]
mod say_tests {
    use super::*;

    fn ports() -> Vec<String> {
        vec!["COM12".into(), "COM24".into(), "/dev/ttyUSB0".into()]
    }

    #[test]
    fn a_port_opens_the_targeted_form() {
        let p = parse_say("COM24 INFO Hello from COM24!", &ports());
        assert_eq!(p.target, Some(Ok("COM24".into())));
        assert_eq!(p.signal.as_deref(), Some("info"));
        assert_eq!(p.text.as_deref(), Some("Hello from COM24!"));
    }

    #[test]
    fn a_signal_without_port_is_broadcast() {
        let p = parse_say("alert.disk Disk at 91%", &ports());
        assert_eq!(p.target, None);
        assert_eq!(p.signal.as_deref(), Some("alert.disk")); // the qualifier names the icon
        assert_eq!(p.text.as_deref(), Some("Disk at 91%"));
    }

    #[test]
    fn bare_prose_is_a_broadcast_transition() {
        let p = parse_say("everything is fine", &ports());
        assert_eq!(p.target, None);
        assert_eq!(p.signal, None);
        assert_eq!(p.text.as_deref(), Some("everything is fine"));
    }

    #[test]
    fn an_unknown_port_is_refused_not_broadcast() {
        let p = parse_say("COM99 INFO hi", &ports());
        assert!(matches!(p.target, Some(Err(_))));
    }

    #[test]
    fn a_unique_suffix_resolves() {
        let p = parse_say("ttyUSB0 INFO hi", &ports());
        assert_eq!(p.target, Some(Ok("/dev/ttyUSB0".into())));
    }
}

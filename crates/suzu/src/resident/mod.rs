//! Resident process composition and domain supervision.
//!
//! Builds the shared event bus and command channels, then starts each
//! domain under supervision.
//!
//! Communication model:
//! - commands: typed per-domain inboxes (`DevicesCmd`, `NotificationCmd`)
//! - events: `ResidentEvent` on one broadcast bus, past tense
//! - read models: `DeviceRow` snapshots via command channels

pub mod admission;
pub mod api;
pub mod device;
pub mod devices;
pub mod events;
pub mod gpu;
pub mod jobs;
pub mod maintenance;
pub mod notifications;
pub mod registry;
pub mod sensor;
pub mod watcher;

use api::Journal;
use devices::{Devices, DevicesCmd, DevicesSnapshot, HostStateCache};
use events::ResidentEvent;
use notifications::{Notifications, NotificationCmd};
use registry::{DeviceRegistry, MaintenanceStep};
use sensor::Sensor;
use std::io::{self, Write};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use crate::Catalog;
use jobs::Jobs;
use tokio::sync::{broadcast, mpsc};

/// Shared event bus and replaceable domain command senders.
pub struct RuntimeChannels {
    events: broadcast::Sender<ResidentEvent>,
    devices: RwLock<mpsc::Sender<DevicesCmd>>,
    notifications: RwLock<mpsc::Sender<NotificationCmd>>,
}

impl RuntimeChannels {
    fn new(events: broadcast::Sender<ResidentEvent>) -> Self {
        let (devices, _) = mpsc::channel(64);
        let (notifications, _) = mpsc::channel(64);
        Self {
            events,
            devices: RwLock::new(devices),
            notifications: RwLock::new(notifications),
        }
    }

    /// Current devices-domain command sender.
    pub fn devices_sender(&self) -> mpsc::Sender<DevicesCmd> {
        self.devices.read().expect("devices sender lock").clone()
    }

    fn set_devices_tx(&self, tx: mpsc::Sender<DevicesCmd>) {
        *self.devices.write().expect("devices sender lock") = tx;
    }

    /// The announcement wire — the bus every client subscribes to.
    pub fn events_sender(&self) -> broadcast::Sender<ResidentEvent> {
        self.events.clone()
    }

    /// Current notifications-domain command sender.
    pub fn notifications_sender(&self) -> mpsc::Sender<NotificationCmd> {
        self.notifications.read().expect("notifications sender lock").clone()
    }

    fn set_notifications_tx(&self, tx: mpsc::Sender<NotificationCmd>) {
        *self.notifications.write().expect("notifications sender lock") = tx;
    }

    /// Submit one display-notification command.
    pub async fn submit_notification(&self, notification: NotificationCmd) {
        let _ = self.notifications_sender().send(notification).await;
    }

    /// Request a bounded snapshot from the devices actor.
    pub async fn snapshot_devices(&self) -> anyhow::Result<DevicesSnapshot> {
        let (tx, mut rx) = mpsc::channel(1);
        self.devices_sender()
            .send(DevicesCmd::Snapshot { reply: tx })
            .await
            .map_err(|_| anyhow::anyhow!("devices domain is not running"))?;
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(snap)) => Ok(snap),
            Ok(None) => anyhow::bail!("devices domain dropped the snapshot"),
            Err(_) => anyhow::bail!("the devices actor did not answer within 5s"),
        }
    }
}

fn line(domain: &str, text: &str) {
    println!("[{domain}] {text}");
    let _ = io::stdout().flush();
}

async fn shutdown_signal() {
    #[cfg(unix)]
    {
        let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("install SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
    }
}

fn log_resident_event(ev: &ResidentEvent, journal: &Journal) {
    let (domain, text) = format_resident_event(ev);
    if text.is_empty() {
        return; // High-frequency data is not journaled.
    }
    line(domain, &text);
    journal.record(domain, &text);
}

/// Format Resident events consistently for the console and journal.
pub(crate) fn format_resident_event(ev: &ResidentEvent) -> (&'static str, String) {
    let say = |domain: &'static str, text: String| (domain, text);
    match ev {
        ResidentEvent::DeviceSensed { port } => say("watcher", format!("sensed {port}")),
        ResidentEvent::DeviceIdentified(f) => {
            let version = f
                .version
                .as_deref()
                .map(|v| format!(" v{v}"))
                .unwrap_or_default();
            say(
                "watcher",
                format!(
                    "identified {} → {} · {}/{}{}",
                    f.port,
                    f.class.as_deref().unwrap_or("no class"),
                    f.family.as_deref().unwrap_or("?"),
                    f.variant.as_deref().unwrap_or("?"),
                    version,
                )
            )
        }
        ResidentEvent::DeviceGone { port } => say("watcher", format!("gone {port}")),
        ResidentEvent::PortBusy { port, reason } => say(
            "watcher",
            format!("{port} is busy — not tracking it ({reason})"),
        ),
        ResidentEvent::DeviceTracked {
            port,
            device_id,
            class,
            state,
        } => say(
            "devices",
            format!(
                "tracking {port} as {class:?} ({device_id:?}) — state {state}"
            ),
        ),
        ResidentEvent::DeviceReconnected { port, device_id } => say(
            "devices",
            format!("reconnected {device_id} on {port}"),
        ),
        ResidentEvent::DeviceReleased { port, device_id } => say(
            "devices",
            format!(
                "released {port} ({device_id:?}) — the registry remembers them"
            ),
        ),
        ResidentEvent::HostMetricsChanged {
            name,
            uptime_s,
            cpu,
            mem,
            disk,
            gpu,
        } => say(
            "sensor",
            format!(
                "metrics: {name} · cpu {cpu}% · gpu {} · mem {mem}% · disk {disk}% · up {uptime_s}s",
                gpu.map_or_else(|| "—".to_string(), |v| format!("{v}%"))
            ),
        ),
        ResidentEvent::DisplayNotificationReady { signal, label, urgency } => say(
            "notifications",
            format!("ring: [{signal}] {label} (urgency {urgency})"),
        ),
        ResidentEvent::Pulse { .. } => ("pulse", String::new()), // High-frequency data is not journaled.
        ResidentEvent::DisplayEventSelected { decision, label } => {
            say("notifications", format!("display event: {decision} {}", label.as_deref().unwrap_or("")))
        }
        ResidentEvent::Degraded { domain, reason } => {
            say(domain, format!("!! degraded: {reason}"))
        }
        ResidentEvent::DeviceRegistered { device_id, port, class } => say(
            "registry",
            format!(
                "registered {device_id} on {port} ({}) — admission determines streaming access",
                class.as_deref().unwrap_or("?")
            ),
        ),
        ResidentEvent::AdmissionReport { device_id, port, passed, steps } => say(
            "registry",
            format!(
                "admission {} for {device_id} on {port} — {}",
                if *passed { "PASSED" } else { "FAILED" },
                steps
                    .iter()
                    .map(|s| format!("{}{}", if s.ok { "" } else { "✗ " }, s.name))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
        ResidentEvent::StreamAttached { device_id, port } => say(
            "registry",
            format!("stream attached — {device_id} on {port}"),
        ),
        ResidentEvent::StreamDetached { device_id, port, reason } => say(
            "registry",
            format!("stream detached — {device_id} on {port} ({reason})"),
        ),
        ResidentEvent::MaintenanceStarted { device_id, port, kind } => say(
            "maintenance",
            format!("{kind} maintenance started for {device_id} on {port}; streaming disabled"),
        ),
        ResidentEvent::MaintenanceStep { device_id, step, index, total, ok, detail } => say(
            "maintenance",
            format!(
                "{device_id} · step {index}/{total} — {step}{} {detail}",
                if *ok { "" } else { " ✗" },
            ),
        ),
        ResidentEvent::Job { job } => say(
            "jobs",
            format!(
                "{} on {} → {} ({}{})",
                job.kind,
                job.target,
                job.label,
                job.state,
                if job.index > 0 { format!(", {} frames", job.index) } else { String::new() }
            ),
        ),
                ResidentEvent::MaintenanceCompleted { device_id, kind, ok } => say(
            "maintenance",
            format!(
                "{kind} maintenance {} for {device_id}; admission controls streaming",
                if *ok { "done" } else { "failed" }
            ),
        ),
        ResidentEvent::Retired { device_id } => say(
            "registry",
            format!("{device_id} retired; its registry entry remains and streaming is disabled"),
        ),
        // Client read-model events are not journaled.
        ResidentEvent::Devices { .. } => ("devices", String::new()),
        ResidentEvent::DeviceRegistry { .. } => ("registry", String::new()),
        ResidentEvent::Frame { .. } => ("media", String::new()),
        ResidentEvent::Snapshot { .. } => ("resident", String::new()),
        ResidentEvent::Paused { paused } => say(
            "devices",
            if *paused {
                "device streaming paused".to_string()
            } else {
                "device streaming resumed".to_string()
            },
        ),
        ResidentEvent::MediaWatched { watched } => say(
            "media",
            if *watched {
                "media capture enabled".to_string()
            } else {
                "media capture disabled".to_string()
            },
        ),
    }
}
//
// The supervised loop owns its domain's command channel: on restart it
// creates a fresh channel and replaces the shared sender.

fn spawn_devices_supervised(
    channels: Arc<RuntimeChannels>,
    rx: mpsc::Receiver<DevicesCmd>,
    registry: Arc<RwLock<DeviceRegistry>>,
    catalog: Arc<Catalog>,
    jobs: Arc<Jobs>,
    host_state: Arc<HostStateCache>,
) {
    tokio::spawn(async move {
        let mut rx = Some(rx);
        let mut backoff = 1u64;
        loop {
            let Some(current) = rx.take() else {
                return;
            };
            let channels2 = Arc::clone(&channels);
            let roster2 = Arc::clone(&registry);
            let catalog2 = Arc::clone(&catalog);
            let jobs2 = Arc::clone(&jobs);
            let host_state2 = Arc::clone(&host_state);
            let bus = channels2.events.subscribe();
            let command_tx = channels2.devices_sender();
            let handle = tokio::spawn(async move {
                Devices::new(channels2.events.clone(), command_tx, catalog2, roster2, jobs2, host_state2)
                    .run(current, bus)
                    .await
            });
            let reason = match handle.await {
                Ok(()) => "command channel closed".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = channels.events.send(ResidentEvent::Degraded {
                domain: "devices",
                reason: reason.clone(),
            });
            line("devices", &format!("!! degraded: {reason} — restarting in {backoff}s"));
            // A restart invalidates every old sender; publish the replacement.
            let (tx, next) = mpsc::channel(64);
            channels.set_devices_tx(tx);
            rx = Some(next);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });
}

fn spawn_notifications_supervised(channels: Arc<RuntimeChannels>, rx: mpsc::Receiver<NotificationCmd>) {
    tokio::spawn(async move {
        let mut rx = Some(rx);
        let mut backoff = 1u64;
        loop {
            let Some(current) = rx.take() else {
                return;
            };
            let channels2 = Arc::clone(&channels);
            let handle = tokio::spawn(async move {
                Notifications::new(channels2.events.clone(), channels2.events.subscribe(), current)
                    .run()
                    .await
            });
            let reason = match handle.await {
                Ok(()) => "command channel closed".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = channels.events.send(ResidentEvent::Degraded {
                domain: "notifications",
                reason: reason.clone(),
            });
            line("notifications", &format!("!! degraded: {reason} — restarting in {backoff}s"));
            let (tx, next) = mpsc::channel(64);
            channels.set_notifications_tx(tx);
            rx = Some(next);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });
}

/// The registry task consumes Resident events and publishes lifecycle state.
/// After each mutation, publish the complete registry (ADR-0004).
fn spawn_roster(channels: Arc<RuntimeChannels>, registry: Arc<RwLock<DeviceRegistry>>) {
    tokio::spawn(async move {
        let mut bus = channels.events.subscribe();
        loop {
            match bus.recv().await {
                Ok(ev) => {
                    let (derived, changed) = process_roster_event(&registry, ev);
                    if changed {
                        let registered_devices = registry
                            .read()
                            .map(|r| r.snapshot())
                            .unwrap_or_default();
                        let _ = channels
                            .events
                            .send(ResidentEvent::DeviceRegistry { registered_devices });
                    }
                    if let Some(d) = derived {
                        let _ = channels.events.send(d);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// Returns the derived verdict event (if any) and whether the registry
/// mutated — a mutation publishes the whole read model.
fn process_roster_event(
    registry: &Arc<RwLock<DeviceRegistry>>,
    ev: ResidentEvent,
) -> (Option<ResidentEvent>, bool) {
    let Ok(mut r) = registry.write() else {
        return (None, false);
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    match ev {
        ResidentEvent::DeviceTracked { port, device_id: Some(id), class, .. } => {
            let changed = r.register(&id, &port, class.as_deref(), &now).is_ok();
            let derived =
                changed.then_some(ResidentEvent::DeviceRegistered { device_id: id, port, class });
            (derived, changed)
        }
        ResidentEvent::AdmissionReport { device_id, port, passed, steps } => {
            let record = registry::AdmissionRecord { passed, at: now, steps };
            match r.admission_result(&device_id, record) {
                Ok(registry::Lifecycle::Live) => (
                    Some(ResidentEvent::StreamAttached { device_id, port }),
                    true,
                ),
                Ok(_) => (
                    Some(ResidentEvent::StreamDetached {
                        device_id,
                        port,
                        reason: "admission failed".into(),
                    }),
                    true,
                ),
                Err(_) => (None, false),
            }
        }
        ResidentEvent::DeviceReleased { port, device_id } => {
            let id = device_id.or_else(|| {
                r.by_port(&port).map(|i| i.device_id.clone())
            });
            let Some(id) = id else { return (None, false) };
            let was_streaming =
                r.registered_device(&id).is_some_and(|i| i.lifecycle == registry::Lifecycle::Live);
            if r.departed(&id).is_err() {
                return (None, false);
            }
            let derived = was_streaming.then(|| ResidentEvent::StreamDetached {
                device_id: id,
                port,
                reason: "departed".into(),
            });
            (derived, true)
        }
        ResidentEvent::MaintenanceStarted { device_id, kind, .. } => {
            let changed = r.maintenance_started(&device_id, &kind).is_ok();
            (None, changed)
        }
        ResidentEvent::MaintenanceStep { device_id, step, index, total, ok, detail } => {
            r.maintenance_step(&device_id, MaintenanceStep { name: step, index, total, ok, detail });
            (None, true)
        }
        ResidentEvent::MaintenanceCompleted { device_id, ok, .. } => {
            let changed = r.maintenance_completed(&device_id, ok).is_ok();
            (None, changed)
        }
        ResidentEvent::Retired { device_id } => {
            let changed = r.retire(&device_id).is_ok();
            (None, changed)
        }
        _ => (None, false),
    }
}

fn spawn_sensor_supervised(channels: Arc<RuntimeChannels>, host_state: Arc<HostStateCache>) {
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            let channels2 = Arc::clone(&channels);
            let host_state2 = Arc::clone(&host_state);
            let handle = tokio::spawn(async move {
                Sensor::new(channels2.events.clone(), host_state2).run().await
            });
            let reason = match handle.await {
                Ok(()) => "sensor loop ended".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = channels.events.send(ResidentEvent::Degraded {
                domain: "sensor",
                reason: reason.clone(),
            });
            line("sensor", &format!("!! degraded: {reason} — restarting in {backoff}s"));
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });
}

pub async fn run(catalog: Arc<Catalog>) -> anyhow::Result<()> {
    // Bind before opening serial ports so a second Resident fails early.
    let listener = match api::bind().await {
        Ok(l) => l,
        Err(e) => {
            let reason = format!(
                "cannot bind 127.0.0.1:{} ({e}); another Suzu Resident may already be running",
                api::API_PORT
            );
            println!("[api] !! {reason}");
            anyhow::bail!("{reason}");
        }
    };
    let (events, _) = broadcast::channel(256);
    let channels = Arc::new(RuntimeChannels::new(events.clone()));
    let registry = Arc::new(RwLock::new(DeviceRegistry::new()));
    let journal = Arc::new(Journal::new());

    // Create command channels once. Supervisors replace them after a restart.
    let (devices_tx, devices_rx) = mpsc::channel(64);
    channels.set_devices_tx(devices_tx);
    let (notifications_tx, notifications_rx) = mpsc::channel(64);
    channels.set_notifications_tx(notifications_tx);

    // watcher
    {
        let watcher = watcher::Watcher {
            links: watcher::WatcherLinks {
                events: channels.events.clone(),
                devices: channels.devices_sender(),
            },
            catalog: Arc::clone(&catalog),
        };
        tokio::spawn(watcher.run());
    }
    let jobs = Arc::new(jobs::Jobs::new(channels.events.clone()));
    let host_state: Arc<HostStateCache> = Arc::default();
    spawn_devices_supervised(Arc::clone(&channels), devices_rx, Arc::clone(&registry), Arc::clone(&catalog), Arc::clone(&jobs), Arc::clone(&host_state));
    spawn_notifications_supervised(Arc::clone(&channels), notifications_rx);
    spawn_sensor_supervised(Arc::clone(&channels), Arc::clone(&host_state));
    spawn_roster(Arc::clone(&channels), Arc::clone(&registry));

    // Local control commands from `suzu pause` and `suzu resume`.
    tokio::spawn(crate::control::listen(channels.devices_sender(), channels.notifications_sender()));

    // Workbench loopback API on the pre-bound listener.
    tokio::spawn(api::listen(Arc::new(api::Ctx {
        catalog: Arc::clone(&catalog),
        jobs: Arc::clone(&jobs),
        events: channels.events_sender(),
        devices: channels.devices_sender(),
        notifications: channels.notifications_sender(),
        registry: Arc::clone(&registry),
        journal: Arc::clone(&journal),
        streams: std::sync::atomic::AtomicUsize::new(0),
    }), listener));

    // Interactive `tell <label>` input.
    let (stdin_tx, mut stdin_rx) = mpsc::channel::<String>(16);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut buf = String::new();
        loop {
            buf.clear();
            match stdin.read_line(&mut buf) {
                Ok(0) | Err(_) => return,
                Ok(_) => {
                    if stdin_tx.blocking_send(buf.trim().to_string()).is_err() {
                        return;
                    }
                }
            }
        }
    });

    println!("suzu resident is up — domains: watcher · devices · notifications · sensor · registry");
    println!("plug in a device to monitor events. Commands: `tell <label>`, `status`, `q`.");
    println!("control: `suzu pause` / `suzu resume` stop and restart the stream to devices.");

    let mut ev_rx = events.subscribe();
    let mut last_ground_line = Option::<std::time::Instant>::None;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                line("resident", "shutdown requested — closing device sessions");
                break;
            },
            ev = ev_rx.recv() => match ev {
                Ok(ev) => {
                    // Rate-limit metrics logging to once every 10 seconds.
                    let throttle = matches!(&ev, ResidentEvent::HostMetricsChanged { .. })
                        && last_ground_line
                            .map(|t| t.elapsed() < Duration::from_secs(10))
                            .unwrap_or(false);
                    if matches!(&ev, ResidentEvent::HostMetricsChanged { .. }) {
                        last_ground_line = Some(std::time::Instant::now());
                    }
                    if !throttle {
                        log_resident_event(&ev, &journal);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    line("resident", &format!("event receiver lagged by {n} events"));
                }
                Err(_) => break,
            },
            Some(input) = stdin_rx.recv() => {
                let input = input.trim();
                match input {
                    "q" | "quit" => break,
                    "" => {}
                    "status" => match channels.snapshot_devices().await {
                        Ok(snap) => {
                            println!(
                                "  stream {} · {} tracked device(s)",
                                if snap.paused { "paused" } else { "active" },
                                snap.devices.len()
                            );
                            for r in snap.devices {
                                let state = format!("{:?}", r.state);
                                println!(
                                    "  {} · {} {}/{} v{} · proto {:?} · {}",
                                    r.port,
                                    r.class.as_deref().unwrap_or("?"),
                                    r.family.as_deref().unwrap_or("?"),
                                    r.variant.as_deref().unwrap_or("?"),
                                    r.version.as_deref().unwrap_or("?"),
                                    r.proto,
                                    state,
                                );
                            }
                        }
                        Err(e) => println!("  snapshot failed: {e}"),
                    },
                    _ if input.starts_with("tell ") => {
                        let label = input.trim_start_matches("tell ").trim();
                        channels
                            .submit_notification(NotificationCmd::submit("console", "transition", Some(label.to_string()), 1))
                            .await;
                    }
                    _ => println!("  ? — tell <label> · status · q"),
                }
            }
        }
    }
    let (reply, mut report) = mpsc::channel(1);
    if channels.devices_sender().send(DevicesCmd::Pause { reply }).await.is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(3), report.recv()).await;
    }
    println!("resident stopped");
    Ok(())
}

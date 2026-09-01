//! The Resident — the DDD monolith that runs the house.
//!
//! Composition root: builds the House (event bus + domain inboxes),
//! spawns every domain under supervision, and (in `suzu serve`) prints
//! the conversation so the correct-way-of-talking is visible.
//!
//! Communication law, enforced by these types:
//! - commands: typed per-domain inboxes (`DevicesCmd`, `MomentsCmd`)
//! - events: `HouseEvent` on one broadcast bus, past tense
//! - cheap objects: `DeviceRow` snapshots via the command door

pub mod admission;
pub mod api;
pub mod device;
pub mod devices;
pub mod events;
pub mod gpu;
pub mod jobs;
pub mod maintenance;
pub mod moments;
pub mod roster;
pub mod sensor;
pub mod watcher;

use api::Journal;
use devices::{Devices, DevicesCmd, DevicesSnapshot, Substrate};
use events::HouseEvent;
use moments::{Moments, MomentsCmd};
use roster::{Roster, SagaStep};
use sensor::Sensor;
use std::io::{self, Write};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use crate::Catalog;
use jobs::Jobs;
use tokio::sync::{broadcast, mpsc};

/// The house wiring. Domains receive `Arc<House>` and may only use
/// these doors.
pub struct House {
    events: broadcast::Sender<HouseEvent>,
    devices: RwLock<mpsc::Sender<DevicesCmd>>,
    moments: RwLock<mpsc::Sender<MomentsCmd>>,
}

impl House {
    fn new(events: broadcast::Sender<HouseEvent>) -> Self {
        let (devices, _) = mpsc::channel(64);
        let (moments, _) = mpsc::channel(64);
        Self {
            events,
            devices: RwLock::new(devices),
            moments: RwLock::new(moments),
        }
    }

    /// The devices door — the read API and the control chirps use it.
    pub fn devices_door(&self) -> mpsc::Sender<DevicesCmd> {
        self.devices.read().expect("house devices lock").clone()
    }

    fn set_devices_tx(&self, tx: mpsc::Sender<DevicesCmd>) {
        *self.devices.write().expect("house devices lock") = tx;
    }

    /// The announcement wire — the bus every client subscribes to.
    pub fn events_door(&self) -> broadcast::Sender<HouseEvent> {
        self.events.clone()
    }

    /// The moments door — visitors speak here.
    pub fn moments_door(&self) -> mpsc::Sender<MomentsCmd> {
        self.moments.read().expect("house moments lock").clone()
    }

    fn set_moments_tx(&self, tx: mpsc::Sender<MomentsCmd>) {
        *self.moments.write().expect("house moments lock") = tx;
    }

    /// The visitor door — one command, from any surface.
    pub async fn tell(&self, moment: MomentsCmd) {
        let _ = self.moments_door().send(moment).await;
    }

    /// Cheap snapshot: a copy, taken by the owning domain. The actor
    /// routes instantly — this is bounded like every other door.
    pub async fn snapshot_devices(&self) -> anyhow::Result<DevicesSnapshot> {
        let (tx, mut rx) = mpsc::channel(1);
        self.devices_door()
            .send(DevicesCmd::Snapshot { reply: tx })
            .await
            .map_err(|_| anyhow::anyhow!("devices domain is not running"))?;
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
            Ok(Some(snap)) => Ok(snap),
            Ok(None) => anyhow::bail!("devices domain dropped the snapshot"),
            Err(_) => anyhow::bail!("the house did not answer within 5s"),
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

fn house_line(ev: &HouseEvent, journal: &Journal) {
    let (domain, text) = format_house_event(ev);
    if text.is_empty() {
        return; // the fast lane is data, not news
    }
    line(domain, &text);
    journal.record(domain, &text);
}

/// The house's facts, in the house's voice — one formatting, shared by
/// the console, the journal and the announcement wire.
pub(crate) fn format_house_event(ev: &HouseEvent) -> (&'static str, String) {
    let say = |domain: &'static str, text: String| (domain, text);
    match ev {
        HouseEvent::DeviceSensed { port } => say("watcher", format!("sensed {port}")),
        HouseEvent::DeviceIdentified(f) => {
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
        HouseEvent::DeviceGone { port } => say("watcher", format!("gone {port}")),
        HouseEvent::PortBusy { port, reason } => say(
            "watcher",
            format!("{port} is busy — not minding ({reason})"),
        ),
        HouseEvent::DeviceMinded {
            port,
            device_id,
            class,
            state,
        } => say(
            "devices",
            format!(
                "minding {port} as {class:?} ({device_id:?}) — state {state}"
            ),
        ),
        HouseEvent::DeviceHomecoming { port, device_id } => say(
            "devices",
            format!("homecoming — {device_id} is back on {port}"),
        ),
        HouseEvent::DeviceReleased { port, device_id } => say(
            "devices",
            format!(
                "released {port} ({device_id:?}) — the roster remembers them"
            ),
        ),
        HouseEvent::GroundChanged {
            name,
            uptime_s,
            cpu,
            mem,
            disk,
            gpu,
        } => say(
            "sensor",
            format!(
                "ground: {name} · cpu {cpu}% · gpu {} · mem {mem}% · disk {disk}% · up {uptime_s}s",
                gpu.map_or_else(|| "—".to_string(), |v| format!("{v}%"))
            ),
        ),
        HouseEvent::Ring { signal, label, urgency } => say(
            "moments",
            format!("ring: [{signal}] {label} (urgency {urgency})"),
        ),
        HouseEvent::Pulse { .. } => ("pulse", String::new()), // the fast lane is data, not news
        HouseEvent::SplashDecided { decision, label } => {
            say("moments", format!("splash: {decision} {}", label.as_deref().unwrap_or("")))
        }
        HouseEvent::Degraded { domain, reason } => {
            say(domain, format!("!! degraded: {reason}"))
        }
        HouseEvent::IndividualHeld { device_id, port, class } => say(
            "roster",
            format!(
                "held {device_id} on {port} ({}) — admission decides the stream",
                class.as_deref().unwrap_or("?")
            ),
        ),
        HouseEvent::AdmissionReport { device_id, port, passed, steps } => say(
            "roster",
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
        HouseEvent::StreamAttached { device_id, port } => say(
            "roster",
            format!("stream attached — {device_id} on {port}"),
        ),
        HouseEvent::StreamDetached { device_id, port, reason } => say(
            "roster",
            format!("stream detached — {device_id} on {port} ({reason})"),
        ),
        HouseEvent::MaintenanceStarted { device_id, port, kind } => say(
            "maintenance",
            format!("{kind} saga owns {device_id} on {port} — the stream is withdrawn"),
        ),
        HouseEvent::MaintenanceStep { device_id, step, index, total, ok, detail } => say(
            "maintenance",
            format!(
                "{device_id} · step {index}/{total} — {step}{} {detail}",
                if *ok { "" } else { " ✗" },
            ),
        ),
        HouseEvent::Job { job } => say(
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
                HouseEvent::MaintenanceCompleted { device_id, kind, ok } => say(
            "maintenance",
            format!(
                "{kind} saga {} for {device_id} — admission decides the stream",
                if *ok { "done" } else { "failed" }
            ),
        ),
        HouseEvent::Retired { device_id } => say(
            "roster",
            format!("{device_id} retired — the roster keeps the name, never the stream"),
        ),
        // ── the wire vocabulary (ADR-0004): data, not news ──────────
        HouseEvent::Devices { .. } => ("devices", String::new()),
        HouseEvent::Roster { .. } => ("roster", String::new()),
        HouseEvent::Frame { .. } => ("media", String::new()),
        HouseEvent::Snapshot { .. } => ("house", String::new()),
        HouseEvent::Paused { paused } => say(
            "devices",
            if *paused {
                "stream paused — the faces fall idle".to_string()
            } else {
                "stream resumed — the faces redress".to_string()
            },
        ),
        HouseEvent::MediaWatched { watched } => say(
            "media",
            if *watched {
                "the media lane is watched — the faces blink for the window".to_string()
            } else {
                "media unwatched — the faces rest their blinks".to_string()
            },
        ),
    }
}
//
// The supervised loop owns its domain's command channel: on restart it
// creates a fresh channel and re-wires the house door, so every sender
// (including the watcher's) lands on the living receiver again.

fn spawn_devices_supervised(
    house: Arc<House>,
    rx: mpsc::Receiver<DevicesCmd>,
    roster: Arc<RwLock<Roster>>,
    catalog: Arc<Catalog>,
    jobs: Arc<Jobs>,
    substrate: Arc<Substrate>,
) {
    tokio::spawn(async move {
        let mut rx = Some(rx);
        let mut backoff = 1u64;
        loop {
            let Some(current) = rx.take() else {
                return;
            };
            let house2 = Arc::clone(&house);
            let roster2 = Arc::clone(&roster);
            let catalog2 = Arc::clone(&catalog);
            let jobs2 = Arc::clone(&jobs);
            let substrate2 = Arc::clone(&substrate);
            let bus = house2.events.subscribe();
            let door = house2.devices_door();
            let handle = tokio::spawn(async move {
                Devices::new(house2.events.clone(), door, catalog2, roster2, jobs2, substrate2)
                    .run(current, bus)
                    .await
            });
            let reason = match handle.await {
                Ok(()) => "command channel closed".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = house.events.send(HouseEvent::Degraded {
                domain: "devices",
                reason: reason.clone(),
            });
            line("devices", &format!("!! degraded: {reason} — restarting in {backoff}s"));
            // A restart invalidates every old sender — re-wire the door.
            let (tx, next) = mpsc::channel(64);
            house.set_devices_tx(tx);
            rx = Some(next);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });
}

fn spawn_moments_supervised(house: Arc<House>, rx: mpsc::Receiver<MomentsCmd>) {
    tokio::spawn(async move {
        let mut rx = Some(rx);
        let mut backoff = 1u64;
        loop {
            let Some(current) = rx.take() else {
                return;
            };
            let house2 = Arc::clone(&house);
            let handle = tokio::spawn(async move {
                Moments::new(house2.events.clone(), house2.events.subscribe(), current)
                    .run()
                    .await
            });
            let reason = match handle.await {
                Ok(()) => "command channel closed".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = house.events.send(HouseEvent::Degraded {
                domain: "moments",
                reason: reason.clone(),
            });
            line("moments", &format!("!! degraded: {reason} — restarting in {backoff}s"));
            let (tx, next) = mpsc::channel(64);
            house.set_moments_tx(tx);
            rx = Some(next);
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });
}

/// The roster's task: it consumes the house's facts and answers with
/// the lifecycle's verdicts. StreamAttached / StreamDetached are its
/// words — the devices domain opens and closes its gates to them.
/// Its read model rides the wire whole (ADR-0004): after every
/// mutation, one `Roster` fact replaces every client's slice — the
/// lifecycle's law lives here, once, never re-derived downstream.
fn spawn_roster(house: Arc<House>, roster: Arc<RwLock<Roster>>) {
    tokio::spawn(async move {
        let mut bus = house.events.subscribe();
        loop {
            match bus.recv().await {
                Ok(ev) => {
                    let (derived, changed) = process_roster_event(&roster, ev);
                    if changed {
                        let individuals = roster
                            .read()
                            .map(|r| r.snapshot())
                            .unwrap_or_default();
                        let _ = house.events.send(HouseEvent::Roster { individuals });
                    }
                    if let Some(d) = derived {
                        let _ = house.events.send(d);
                    }
                }
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
    });
}

/// Returns the derived verdict event (if any) and whether the roster
/// mutated — a mutation publishes the whole read model.
fn process_roster_event(
    roster: &Arc<RwLock<Roster>>,
    ev: HouseEvent,
) -> (Option<HouseEvent>, bool) {
    let Ok(mut r) = roster.write() else {
        return (None, false);
    };
    let now = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
    match ev {
        HouseEvent::DeviceMinded { port, device_id: Some(id), class, .. } => {
            let changed = r.hold(&id, &port, class.as_deref(), &now).is_ok();
            let derived = changed.then_some(HouseEvent::IndividualHeld { device_id: id, port, class });
            (derived, changed)
        }
        HouseEvent::AdmissionReport { device_id, port, passed, steps } => {
            let record = roster::AdmissionRecord { passed, at: now, steps };
            match r.admission_result(&device_id, record) {
                Ok(roster::Lifecycle::Live) => (
                    Some(HouseEvent::StreamAttached { device_id, port }),
                    true,
                ),
                Ok(_) => (
                    Some(HouseEvent::StreamDetached {
                        device_id,
                        port,
                        reason: "admission failed".into(),
                    }),
                    true,
                ),
                Err(_) => (None, false),
            }
        }
        HouseEvent::DeviceReleased { port, device_id } => {
            let id = device_id.or_else(|| {
                r.by_port(&port).map(|i| i.device_id.clone())
            });
            let Some(id) = id else { return (None, false) };
            let was_streaming =
                r.individual(&id).is_some_and(|i| i.lifecycle == roster::Lifecycle::Live);
            if r.departed(&id).is_err() {
                return (None, false);
            }
            let derived = was_streaming.then(|| HouseEvent::StreamDetached {
                device_id: id,
                port,
                reason: "departed".into(),
            });
            (derived, true)
        }
        HouseEvent::MaintenanceStarted { device_id, kind, .. } => {
            let changed = r.maintenance_started(&device_id, &kind).is_ok();
            (None, changed)
        }
        HouseEvent::MaintenanceStep { device_id, step, index, total, ok, detail } => {
            r.maintenance_step(&device_id, SagaStep { name: step, index, total, ok, detail });
            (None, true)
        }
        HouseEvent::MaintenanceCompleted { device_id, ok, .. } => {
            let changed = r.maintenance_completed(&device_id, ok).is_ok();
            (None, changed)
        }
        HouseEvent::Retired { device_id } => {
            let changed = r.retire(&device_id).is_ok();
            (None, changed)
        }
        _ => (None, false),
    }
}

fn spawn_sensor_supervised(house: Arc<House>, substrate: Arc<Substrate>) {
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            let house2 = Arc::clone(&house);
            let substrate2 = Arc::clone(&substrate);
            let handle = tokio::spawn(async move {
                Sensor::new(house2.events.clone(), substrate2).run().await
            });
            let reason = match handle.await {
                Ok(()) => "sensor loop ended".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = house.events.send(HouseEvent::Degraded {
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
    // The door first (ADR-0004): the resident binds 7899 before it
    // touches any serial port, and a second claimant exits loudly with
    // a reason — never doorless, never a zombie watching ports.
    let listener = match api::bind().await {
        Ok(l) => l,
        Err(e) => {
            let reason = format!(
                "cannot claim the door 127.0.0.1:{} ({e}) — another suzu resident already owns the house",
                api::API_PORT
            );
            println!("[api] !! {reason}");
            anyhow::bail!("{reason}");
        }
    };
    let (events, _) = broadcast::channel(256);
    let house = Arc::new(House::new(events.clone()));
    let roster = Arc::new(RwLock::new(Roster::new()));
    let journal = Arc::new(Journal::new());

    // Create the domain doors once; the supervised loops only recreate
    // them on restart (and re-wire the house at that moment), so every
    // sender — including the watcher's handoff — stays live.
    let (devices_tx, devices_rx) = mpsc::channel(64);
    house.set_devices_tx(devices_tx);
    let (moments_tx, moments_rx) = mpsc::channel(64);
    house.set_moments_tx(moments_tx);

    // watcher
    {
        let watcher = watcher::Watcher {
            links: watcher::WatcherLinks {
                events: house.events.clone(),
                devices: house.devices_door(),
            },
            catalog: Arc::clone(&catalog),
        };
        tokio::spawn(watcher.run());
    }
    let jobs = Arc::new(jobs::Jobs::new(house.events.clone()));
    let substrate: Arc<Substrate> = Arc::default();
    spawn_devices_supervised(Arc::clone(&house), devices_rx, Arc::clone(&roster), Arc::clone(&catalog), Arc::clone(&jobs), Arc::clone(&substrate));
    spawn_moments_supervised(Arc::clone(&house), moments_rx);
    spawn_sensor_supervised(Arc::clone(&house), Arc::clone(&substrate));
    spawn_roster(Arc::clone(&house), Arc::clone(&roster));

    // the control chirp: `suzu pause` / `suzu resume` from any shell
    tokio::spawn(crate::control::listen(house.devices_door(), house.moments_door()));

    // the workbench's door: the loopback read API (ADR-0002), on the
    // listener claimed before the house was built (ADR-0004)
    tokio::spawn(api::listen(Arc::new(api::Ctx {
        catalog: Arc::clone(&catalog),
        jobs: Arc::clone(&jobs),
        events: house.events_door(),
        devices: house.devices_door(),
        moments: house.moments_door(),
        roster: Arc::clone(&roster),
        journal: Arc::clone(&journal),
        streams: std::sync::atomic::AtomicUsize::new(0),
    }), listener));

    // the visitor door, by hand: `tell <label>`
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

    println!("suzu resident is up — domains: watcher · devices · moments · sensor · roster");
    println!("plug a device and watch the house talk. `tell <label>` rings the bell · `status` · `q` quits.");
    println!("control: `suzu pause` / `suzu resume` stop and restart the stream to devices.");

    let mut ev_rx = events.subscribe();
    let mut last_ground_line = Option::<std::time::Instant>::None;
    let shutdown = shutdown_signal();
    tokio::pin!(shutdown);
    loop {
        tokio::select! {
            _ = &mut shutdown => {
                line("house", "shutdown requested — releasing the companions");
                break;
            },
            ev = ev_rx.recv() => match ev {
                Ok(ev) => {
                    // Ground drifts silently — log it at most every 10 s
                    // so the conversation stays readable.
                    let throttle = matches!(&ev, HouseEvent::GroundChanged { .. })
                        && last_ground_line
                            .map(|t| t.elapsed() < Duration::from_secs(10))
                            .unwrap_or(false);
                    if matches!(&ev, HouseEvent::GroundChanged { .. }) {
                        last_ground_line = Some(std::time::Instant::now());
                    }
                    if !throttle {
                        house_line(&ev, &journal);
                    }
                }
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    line("house", &format!("lagged by {n} events"));
                }
                Err(_) => break,
            },
            Some(input) = stdin_rx.recv() => {
                let input = input.trim();
                match input {
                    "q" | "quit" => break,
                    "" => {}
                    "status" => match house.snapshot_devices().await {
                        Ok(snap) => {
                            println!(
                                "  stream {} · {} minded device(s)",
                                if snap.paused { "paused" } else { "flowing" },
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
                        house
                            .tell(MomentsCmd::tell("keeper", "transition", Some(label.to_string()), 1))
                            .await;
                    }
                    _ => println!("  ? — tell <label> · status · q"),
                }
            }
        }
    }
    let (reply, mut report) = mpsc::channel(1);
    if house.devices_door().send(DevicesCmd::Pause { reply }).await.is_ok() {
        let _ = tokio::time::timeout(Duration::from_secs(3), report.recv()).await;
    }
    println!("the resident rests — the garden keeps breathing.");
    Ok(())
}

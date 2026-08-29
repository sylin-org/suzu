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

pub mod devices;
pub mod events;
pub mod gpu;
pub mod moments;
pub mod publisher;
pub mod sensor;
pub mod watcher;

use devices::{DeviceRow, Devices, DevicesCmd};
use events::HouseEvent;
use moments::{Moments, MomentsCmd};
use sensor::Sensor;
use std::io::{self, Write};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use crate::Catalog;
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

    fn devices_tx(&self) -> mpsc::Sender<DevicesCmd> {
        self.devices.read().expect("house devices lock").clone()
    }

    fn set_devices_tx(&self, tx: mpsc::Sender<DevicesCmd>) {
        *self.devices.write().expect("house devices lock") = tx;
    }

    fn moments_tx(&self) -> mpsc::Sender<MomentsCmd> {
        self.moments.read().expect("house moments lock").clone()
    }

    fn set_moments_tx(&self, tx: mpsc::Sender<MomentsCmd>) {
        *self.moments.write().expect("house moments lock") = tx;
    }

    /// The visitor door — one command, from any surface.
    pub async fn tell(&self, moment: MomentsCmd) {
        let _ = self.moments_tx().send(moment).await;
    }

    /// Cheap snapshot: a copy, taken by the owning domain.
    pub async fn snapshot_devices(&self) -> anyhow::Result<Vec<DeviceRow>> {
        let (tx, mut rx) = mpsc::channel(1);
        self.devices_tx()
            .send(DevicesCmd::Snapshot { reply: tx })
            .await
            .map_err(|_| anyhow::anyhow!("devices domain is not running"))?;
        rx.recv()
            .await
            .ok_or_else(|| anyhow::anyhow!("devices domain dropped the snapshot"))
    }
}

fn line(domain: &str, text: &str) {
    println!("[{domain}] {text}");
    let _ = io::stdout().flush();
}

fn house_line(ev: &HouseEvent) {
    match ev {
        HouseEvent::DeviceSensed { port } => line("watcher", &format!("sensed {port}")),
        HouseEvent::DeviceIdentified(f) => {
            let version = f
                .version
                .as_deref()
                .map(|v| format!(" v{v}"))
                .unwrap_or_default();
            line(
                "watcher",
                &format!(
                    "identified {} → {} · {}/{}{}",
                    f.port,
                    f.class.as_deref().unwrap_or("no class"),
                    f.family.as_deref().unwrap_or("?"),
                    f.variant.as_deref().unwrap_or("?"),
                    version,
                ),
            );
        }
        HouseEvent::DeviceGone { port } => line("watcher", &format!("gone {port}")),
        HouseEvent::PortBusy { port, reason } => line(
            "watcher",
            &format!("{port} is busy — not minding ({reason})"),
        ),
        HouseEvent::DeviceMinded {
            port,
            device_id,
            class,
            state,
        } => line(
            "devices",
            &format!(
                "minding {port} as {class:?} ({device_id:?}) — state {state}"
            ),
        ),
        HouseEvent::DeviceHomecoming { port, device_id } => line(
            "devices",
            &format!("homecoming — {device_id} is back on {port}"),
        ),
        HouseEvent::DeviceReleased { port, device_id } => line(
            "devices",
            &format!(
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
        } => line(
            "sensor",
            &format!(
                "ground: {name} · cpu {cpu}% · gpu {} · mem {mem}% · disk {disk}% · up {uptime_s}s",
                gpu.map_or_else(|| "—".to_string(), |v| format!("{v}%"))
            ),
        ),
        HouseEvent::Ring { label, urgency } => line(
            "moments",
            &format!("ring: {label} (urgency {urgency})"),
        ),
        HouseEvent::Pulse { .. } => {} // the pulse lane is silent by design
        HouseEvent::SplashDecided { decision, label } => {
            line("moments", &format!("splash: {decision} {}", label.as_deref().unwrap_or("")))
        }
        HouseEvent::Degraded { domain, reason } => {
            line(domain, &format!("!! degraded: {reason}"))
        }
    }
}

// ── supervision — a domain reports before it trips, then restarts ──
//
// The supervised loop owns its domain's command channel: on restart it
// creates a fresh channel and re-wires the house door, so every sender
// (including the watcher's) lands on the living receiver again.

fn spawn_devices_supervised(house: Arc<House>, rx: mpsc::Receiver<DevicesCmd>) {
    tokio::spawn(async move {
        let mut rx = Some(rx);
        let mut backoff = 1u64;
        loop {
            let Some(current) = rx.take() else {
                return;
            };
            let house2 = Arc::clone(&house);
            let handle =
                tokio::spawn(async move { Devices::new(house2.events.clone()).run(current).await });
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

fn spawn_sensor_supervised(house: Arc<House>) {
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            let house2 = Arc::clone(&house);
            let handle = tokio::spawn(async move { Sensor::new(house2.events.clone()).run().await });
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

fn spawn_publisher_supervised(house: Arc<House>) {
    tokio::spawn(async move {
        let mut backoff = 1u64;
        loop {
            let house2a = Arc::clone(&house);
            let house2b = Arc::clone(&house);
            let handle = tokio::spawn(async move {
                publisher::Publisher::new(house2a, house2b.events.subscribe())
                    .run()
                    .await
            });
            let reason = match handle.await {
                Ok(()) => "publisher loop ended".to_string(),
                Err(e) => format!("panic: {e}"),
            };
            let _ = house.events.send(HouseEvent::Degraded {
                domain: "publisher",
                reason: reason.clone(),
            });
            line("publisher", &format!("!! degraded: {reason} — restarting in {backoff}s"));
            tokio::time::sleep(Duration::from_secs(backoff)).await;
            backoff = (backoff * 2).min(30);
        }
    });
}

pub async fn run(catalog: Arc<Catalog>) -> anyhow::Result<()> {
    let (events, _) = broadcast::channel(128);
    let house = Arc::new(House::new(events.clone()));

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
                devices: house.devices_tx(),
            },
            catalog,
        };
        tokio::spawn(watcher.run());
    }
    spawn_devices_supervised(Arc::clone(&house), devices_rx);
    spawn_moments_supervised(Arc::clone(&house), moments_rx);
    spawn_sensor_supervised(Arc::clone(&house));
    spawn_publisher_supervised(Arc::clone(&house));

    // the control chirp: `suzu pause` / `suzu resume` from any shell
    tokio::spawn(crate::control::listen(house.devices_tx(), house.moments_tx()));

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

    println!("suzu resident is up — domains: watcher · devices · moments · sensor · publisher");
    println!("plug a device and watch the house talk. `tell <label>` rings the bell · `status` · `q` quits.");
    println!("control: `suzu pause` / `suzu resume` stop and restart the stream to devices.");

    let mut ev_rx = events.subscribe();
    let mut last_ground_line = Option::<std::time::Instant>::None;
    loop {
        tokio::select! {
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
                        house_line(&ev);
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
                        Ok(rows) => {
                            if rows.is_empty() {
                                println!("  no minded devices");
                            }
                            for r in rows {
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
    println!("the resident rests — the garden keeps breathing.");
    Ok(())
}

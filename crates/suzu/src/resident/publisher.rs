//! The publisher domain — picks up the sensor's published surface and
//! distributes it across all consumers.
//!
//! On `GroundChanged` (the sensor's small drift event) the publisher
//! hands the full published object to the devices domain as a cheap
//! `Arc` copy; each live device's consumer translates it to the surface
//! its limitations accept and pushes device-shaped data to the device.

use super::events::HouseEvent;
use super::sensor::MachineReport;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

pub struct Publisher {
    /// The house door — looked up fresh on every publish, so a devices
    /// domain restart re-wires the pipeline automatically.
    house: Arc<super::House>,
    events: Receiver<HouseEvent>,
    last: Option<Arc<MachineReport>>,
}

impl Publisher {
    pub fn new(house: Arc<super::House>, events: Receiver<HouseEvent>) -> Self {
        Self {
            house,
            events,
            last: None,
        }
    }

    pub async fn run(mut self) {
        loop {
            let ev = tokio::select! {
                ev = self.events.recv() => match ev {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Missed drift events — publish the next one we see.
                        eprintln!("[publisher] lagged by {n} events");
                        continue;
                    }
                    Err(_) => return,
                },
                _ = tokio::time::sleep(Duration::from_secs(3600)) => continue,
            };
            match ev {
                HouseEvent::GroundChanged {
                    name,
                    uptime_s,
                    cpu,
                    mem,
                    disk,
                    gpu,
                } => {
                    let ground = Arc::new(MachineReport {
                        name,
                        uptime_s,
                        cpu,
                        mem,
                        disk,
                        gpu,
                    });
                    self.last = Some(Arc::clone(&ground));
                    self.distribute(ground).await;
                }
                HouseEvent::Pulse { axis, value } => {
                    // Fast lane: forward to consumers that declared it.
                    let _ = self
                        .house
                        .devices_door()
                        .send(super::devices::DevicesCmd::Pulse { axis: axis.to_string(), value })
                        .await;
                }
                                HouseEvent::DeviceMinded { .. } => {
                    // A new consumer appeared — give it the current
                    // published object immediately, so its face fills
                    // without waiting for the next drift.
                    if let Some(ground) = &self.last {
                        self.distribute(Arc::clone(ground)).await;
                    }
                }
                HouseEvent::Ring { signal, label, urgency } => {
                    let _ = self
                        .house
                        .devices_door()
                        .send(super::devices::DevicesCmd::Ring { signal, label, urgency })
                        .await;
                }
                _ => {}
            }
        }
    }

    async fn distribute(&self, ground: Arc<MachineReport>) {
        let _ = self
            .house
            .devices_door()
            .send(super::devices::DevicesCmd::Publish(ground))
            .await;
    }
}

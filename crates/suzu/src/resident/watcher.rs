//! The watcher domain — senses USB arrivals and departures, runs the
//! identification ladder, hands the facts to the devices domain, and
//! ends its cycle. It never manages; it keeps only the presence ear.

use super::events::{DeviceFacts, HouseEvent};
use crate::catalog::Catalog;
use std::sync::Arc;
use crate::probe;
use serialport::SerialPortType;
use std::collections::HashSet;
use std::time::Duration;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc;

/// What the watcher needs to reach the devices domain.
#[derive(Clone)]
pub struct WatcherLinks {
    pub events: Sender<HouseEvent>,
    pub devices: mpsc::Sender<super::devices::DevicesCmd>,
}

pub struct Watcher {
    pub links: WatcherLinks,
    pub catalog: Arc<Catalog>,
}

fn usb_of(p: &serialport::SerialPortInfo) -> Option<(u16, u16)> {
    match &p.port_type {
        SerialPortType::UsbPort(u) => Some((u.vid, u.pid)),
        _ => None,
    }
}

/// One identification pass, blocking — run inside spawn_blocking.
/// `Err(reason)` means the port could not be honestly read (busy,
/// stale, or failing) — such ports are never minded.
pub fn identify_facts(catalog: &Catalog, port: &str, vid: u16, pid: u16) -> Result<DeviceFacts, String> {
    let t = probe::probe_transcript(port);
    if let Some(err) = &t.error {
        return Err(err.clone());
    }
    let class_by_sig = t.identity.as_ref().and_then(|j| {
        let f = j.get("family").and_then(|v| v.as_str())?;
        let var = j.get("variant").and_then(|v| v.as_str())?;
        catalog.class_by_signature(f, var).map(|c| c.id.clone())
    });
    let class_by_vp = catalog
        .class_by_vidpid(vid, pid)
        .map(|c| c.id.clone())
        .or_else(|| crate::catalog::seed_class_for(vid, pid));
    let class = class_by_sig.or(class_by_vp);
    let field = |k: &str| {
        t.identity
            .as_ref()
            .and_then(|j| j.get(k))
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    };
    let device_id = t.identity.as_ref().and_then(|j| {
        j.get("device_id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
    });
    Ok(DeviceFacts {
        port: port.to_string(),
        vid,
        pid,
        class,
        family: field("family"),
        variant: field("variant"),
        version: field("version"),
        proto: field("proto"),
        device_id,
        legacy: t.legacy_line.is_some(),
    })
}

impl Watcher {
    pub async fn run(self) {
        let mut seen: HashSet<String> = HashSet::new();
        // Ports whose identification failed: retried every cycle, with
        // the failure logged once (not per retry). A busy or wedged
        // port is a moment, not a verdict — tonight's lesson: a port
        // marked seen before a panicking probe never came back.
        let mut failed: HashSet<String> = HashSet::new();
        loop {
            let ports = match tokio::task::spawn_blocking(serialport::available_ports).await {
                Ok(Ok(ports)) => ports,
                _ => {
                    tokio::time::sleep(Duration::from_millis(1000)).await;
                    continue;
                }
            };

            let mut present: HashSet<String> = HashSet::new();
            for p in &ports {
                present.insert(p.port_name.clone());
                let Some((vid, pid)) = usb_of(p) else {
                    continue; // non-USB — foreign by default, not our watch
                };
                if seen.contains(&p.port_name) {
                    continue;
                }
                seen.insert(p.port_name.clone());
                // `insert` answers false on retries: the first attempt
                // announces itself, later retries stay quiet.
                let first_attempt = failed.insert(p.port_name.clone());
                if first_attempt {
                    let _ = self.links.events.send(HouseEvent::DeviceSensed {
                        port: p.port_name.clone(),
                    });
                }

                // The ladder blocks (up to ~5.5 s) — off the async lane.
                let catalog = self.catalog.clone();
                let name = p.port_name.clone();
                let identified = tokio::task::spawn_blocking(move || {
                    identify_facts(&catalog, &name, vid, pid)
                })
                .await
                .unwrap_or_else(|e| Err(format!("join: {e}")));

                // Report-before-minding: an unreachable port is a fact
                // for the house, never a device to mind.
                let facts = match identified {
                    Ok(facts) => {
                        failed.remove(&p.port_name);
                        facts
                    }
                    Err(reason) => {
                        // Back in the queue — the next cycle retries.
                        seen.remove(&p.port_name);
                        if first_attempt {
                            let _ = self.links.events.send(HouseEvent::PortBusy {
                                port: p.port_name.clone(),
                                reason,
                            });
                        }
                        continue;
                    }
                };

                let _ = self
                    .links
                    .events
                    .send(HouseEvent::DeviceIdentified(facts.clone()));
                // Handoff: the watcher asks the devices domain to mind
                // the new device, then ends its cycle for this port.
                let _ = self
                    .links
                    .devices
                    .send(super::devices::DevicesCmd::Mind(facts))
                    .await;
            }

            for gone in seen.difference(&present).cloned().collect::<Vec<_>>() {
                seen.remove(&gone);
                failed.remove(&gone);
                let _ = self.links.events.send(HouseEvent::DeviceGone {
                    port: gone.clone(),
                });
                let _ = self.links.devices.send(super::devices::DevicesCmd::Gone { port: gone }).await;
            }

            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }
}

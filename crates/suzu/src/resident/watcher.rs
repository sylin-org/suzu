//! USB device discovery and identification.
//!
//! This module detects serial-port changes, identifies candidate devices,
//! and forwards the resulting facts to the device manager.

use super::events::{DeviceFacts, ResidentEvent};
use crate::catalog::Catalog;
use std::sync::Arc;
use crate::probe;
use serialport::SerialPortType;
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::broadcast::Sender;
use tokio::sync::mpsc;

/// Channels used by device discovery.
#[derive(Clone)]
pub struct WatcherLinks {
    pub events: Sender<ResidentEvent>,
    pub devices: mpsc::Sender<super::devices::DevicesCmd>,
}

/// Number of identification attempts allowed while a known device boots.
const BOOT_PATIENCE: u32 = 5;

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
/// `Err(reason)` means the port could not be read (busy,
/// stale, or failing); such ports are not tracked.
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
        faceplate: field("faceplate"),
        mount: field("mount"),
        device_id,
        legacy: t.legacy_line.is_some(),
    })
}

impl Watcher {
    pub async fn run(self) {
        let mut seen: HashSet<String> = HashSet::new();
        // Retry ports whose identification failed on every cycle, but log each
        // failure only once. Do not mark failed probes as successfully seen.
        let mut failed: HashSet<String> = HashSet::new();
        // A known-class port may need several seconds to boot and compile source.
        // Retry it up to BOOT_PATIENCE times before classifying its firmware as
        // unknown.
        let mut booting: HashMap<String, u32> = HashMap::new();
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
                    continue; // Non-USB ports are outside device discovery.
                };
                if seen.contains(&p.port_name) {
                    continue;
                }
                // Skip ports reserved by a maintenance procedure.
                // A probe's DTR/RTS toggle would reset the board during an
                // installation. Skip the port until maintenance releases it.
                if super::devices::reserved_ports()
                    .lock()
                    .unwrap()
                    .contains(&p.port_name)
                {
                    continue;
                }
                seen.insert(p.port_name.clone());
                // `insert` answers false on retries: the first attempt
                // Log the first failure only; later retries remain silent.
                let first_attempt = failed.insert(p.port_name.clone());
                if first_attempt {
                    let _ = self.links.events.send(ResidentEvent::DeviceSensed {
                        port: p.port_name.clone(),
                    });
                }

                // Identification can block for about 5.5 seconds, so run it off the async loop.
                let catalog = self.catalog.clone();
                let name = p.port_name.clone();
                let identified = tokio::task::spawn_blocking(move || {
                    identify_facts(&catalog, &name, vid, pid)
                })
                .await
                .unwrap_or_else(|e| Err(format!("join: {e}")));

                // A silent descriptor from a board the catalog knows by
                // vid/pid is a face mid-boot, not a firmware verdict —
                // one settled re-probe before believing the silence.
                let identified = match identified {
                    Ok(facts)
                        if facts.proto.is_none()
                            && facts.class.is_some()
                            && !facts.legacy =>
                    {
                        tokio::time::sleep(Duration::from_millis(2500)).await;
                        let catalog = self.catalog.clone();
                        let name2 = p.port_name.clone();
                        tokio::task::spawn_blocking(move || {
                            identify_facts(&catalog, &name2, vid, pid)
                        })
                        .await
                        .unwrap_or_else(|e| Err(format!("join: {e}")))
                    }
                    other => other,
                };

                // Report-before-minding: an unreachable port is a fact
                // ignored by the Resident. A known class that
                // catalog knows runs suzu — a silent descriptor is a
                // face mid-boot, not a firmware verdict: settled
                // re-probes until patience runs out, then belief.
                let facts = match identified {
                    Ok(facts) if facts.proto.is_none() && facts.class.is_some() => {
                        let tries = booting.entry(p.port_name.clone()).or_insert(0);
                        *tries += 1;
                        if *tries <= BOOT_PATIENCE {
                            seen.remove(&p.port_name); // back in the queue
                            continue;
                        }
                        booting.remove(&p.port_name);
                        failed.remove(&p.port_name);
                        facts // confirmed silent: track as unknown firmware
                    }
                    Ok(facts) => {
                        booting.remove(&p.port_name);
                        failed.remove(&p.port_name);
                        facts
                    }
                    Err(reason) => {
                        // Back in the queue — the next cycle retries.
                        booting.remove(&p.port_name);
                        seen.remove(&p.port_name);
                        if first_attempt {
                            let _ = self.links.events.send(ResidentEvent::PortBusy {
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
                    .send(ResidentEvent::DeviceIdentified(facts.clone()));
                // Handoff: the watcher asks the devices domain to mind
                // the new device, then ends its cycle for this port.
                let _ = self
                    .links
                    .devices
                    .send(super::devices::DevicesCmd::Track(facts))
                    .await;
            }

            for gone in seen.difference(&present).cloned().collect::<Vec<_>>() {
                seen.remove(&gone);
                failed.remove(&gone);
                let _ = self.links.events.send(ResidentEvent::DeviceGone {
                    port: gone.clone(),
                });
                let _ = self.links.devices.send(super::devices::DevicesCmd::Gone { port: gone }).await;
            }

            tokio::time::sleep(Duration::from_millis(1000)).await;
        }
    }
}

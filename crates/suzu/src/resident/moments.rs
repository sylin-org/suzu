//! The moments domain — the visitor door, the budget, and tenancy.
//!
//! Device events are the moments source (`appeared` rings discovery,
//! `gone` rings the toll); visitors (agents, producers, `suzu tell`)
//! hand moments through the door. Everything the face should show is
//! decided here and published as SplashDecided.

use super::events::HouseEvent;
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize)]
pub struct Moment {
    pub from: String,
    pub kind: String, // transition · discovery · completion · alert · …
    pub label: Option<String>,
    pub urgency: u8, // 0–5, the vitality scale's tempo gloss
}

pub enum MomentsCmd {
    Tell(Moment),
}

impl MomentsCmd {
    /// The visitor door, in one call.
    pub fn tell(from: &str, kind: &str, label: Option<String>, urgency: u8) -> Self {
        Self::Tell(Moment {
            from: from.to_string(),
            kind: kind.to_string(),
            label,
            urgency,
        })
    }
}

const MIN_SPLASH_INTERVAL: Duration = Duration::from_secs(2);

pub struct Moments {
    events_tx: Sender<HouseEvent>,
    events: Receiver<HouseEvent>,
    cmd: mpsc::Receiver<MomentsCmd>,
    last_splash: Option<Instant>,
    coalesced: u32,
}

impl Moments {
    pub fn new(
        events_tx: Sender<HouseEvent>,
        events: Receiver<HouseEvent>,
        cmd: mpsc::Receiver<MomentsCmd>,
    ) -> Self {
        Self {
            events_tx,
            events,
            cmd,
            last_splash: None,
            coalesced: 0,
        }
    }

    fn splash(&mut self, decision: &str, label: Option<String>) {
        let _ = self
            .events_tx
            .send(HouseEvent::SplashDecided {
                decision: decision.to_string(),
                label,
            });
    }

    /// The budget: bursts coalesce into one longer visit.
    fn within_budget(&mut self) -> bool {
        match self.last_splash {
            None => true,
            Some(t) if t.elapsed() >= MIN_SPLASH_INTERVAL => true,
            Some(_) => {
                self.coalesced += 1;
                false
            }
        }
    }

    pub async fn run(mut self) {
        loop {
            tokio::select! {
                ev = self.events.recv() => match ev {
                    Ok(HouseEvent::DeviceMinded { port, device_id, .. }) => {
                        // A new individual in the house → discovery.
                        self.splash(
                            "discovery",
                            Some(device_id.unwrap_or(port)),
                        );
                    }
                    Ok(HouseEvent::DeviceReleased { port, device_id }) => {
                        // The toll — a departure leaves the room.
                        self.splash(
                            "departure",
                            Some(device_id.unwrap_or(port)),
                        );
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // Lagged reads are lost moments — budget protects
                        // the face; log goes to the house's own channel.
                        eprintln!("[moments] lagged by {n} events");
                    }
                    Err(_) => return,
                },
                cmd = self.cmd.recv() => match cmd {
                    Some(MomentsCmd::Tell(moment)) => {
                        if self.within_budget() {
                            let label = moment.label.clone();
                            let n = self.coalesced;
                            if n > 0 {
                                self.coalesced = 0;
                                self.splash("coalesced-splash", Some(format!("{n} earlier notes + {}", label.unwrap_or_default())));
                            } else {
                                self.splash(&format!("{} ({}, urgency {})", moment.kind, moment.from, moment.urgency), label);
                            }
                            self.last_splash = Some(Instant::now());
                        } else {
                            self.coalesced += 1;
                            self.splash("coalesced", None);
                        }
                    }
                    None => return,
                },
            }
        }
    }
}

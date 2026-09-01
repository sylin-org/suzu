//! Display-event routing and rate limiting.
//!
//! Device lifecycle events and external producers submit display events through
//! this module. It rate-limits notifications and publishes each selection as
//! `DisplayEventSelected`.

use super::events::ResidentEvent;
use serde::Serialize;
use std::time::{Duration, Instant};
use tokio::sync::broadcast::{Receiver, Sender};
use tokio::sync::mpsc;

#[derive(Debug, Clone, Serialize)]
pub struct DisplayNotification {
    pub from: String,
    pub kind: String, // transition · discovery · completion · alert · …
    pub label: Option<String>,
    pub urgency: u8, // Protocol urgency from 0 through 5.
}

pub enum NotificationCmd {
    Submit(DisplayNotification),
}

impl NotificationCmd {
    /// Submit one display event.
    pub fn submit(from: &str, kind: &str, label: Option<String>, urgency: u8) -> Self {
        Self::Submit(DisplayNotification {
            from: from.to_string(),
            kind: kind.to_string(),
            label,
            urgency,
        })
    }
}

const MIN_EVENT_INTERVAL: Duration = Duration::from_secs(2);

pub struct Notifications {
    events_tx: Sender<ResidentEvent>,
    events: Receiver<ResidentEvent>,
    cmd: mpsc::Receiver<NotificationCmd>,
    last_event: Option<Instant>,
    coalesced: u32,
}

impl Notifications {
    pub fn new(
        events_tx: Sender<ResidentEvent>,
        events: Receiver<ResidentEvent>,
        cmd: mpsc::Receiver<NotificationCmd>,
    ) -> Self {
        Self {
            events_tx,
            events,
            cmd,
            last_event: None,
            coalesced: 0,
        }
    }

    fn publish_selection(&mut self, decision: &str, label: Option<String>) {
        let _ = self
            .events_tx
            .send(ResidentEvent::DisplayEventSelected {
                decision: decision.to_string(),
                label,
            });
    }

    /// Coalesce bursts that exceed the minimum event interval.
    fn within_budget(&mut self) -> bool {
        match self.last_event {
            None => true,
            Some(t) if t.elapsed() >= MIN_EVENT_INTERVAL => true,
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
                    Ok(ResidentEvent::DeviceTracked { port, device_id, .. }) => {
                        // A newly tracked device produces a discovery event.
                        self.publish_selection(
                            "discovery",
                            Some(device_id.unwrap_or(port)),
                        );
                    }
                    Ok(ResidentEvent::DeviceReleased { port, device_id }) => {
                        // Publish a departure notification.
                        self.publish_selection(
                            "departure",
                            Some(device_id.unwrap_or(port)),
                        );
                    }
                    Ok(_) => {}
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        // The receiver lost events because it lagged. Keep the
                        // device rate limit active and report the count.
                        eprintln!("[notifications] lagged by {n} events");
                    }
                    Err(_) => return,
                },
                cmd = self.cmd.recv() => match cmd {
                    Some(NotificationCmd::Submit(notification)) => {
                        if self.within_budget() {
                            let label = notification.label.clone();
                            let n = self.coalesced;
                            if n > 0 {
                                self.coalesced = 0;
                                let text = format!("{n} earlier notes + {}", label.unwrap_or_default());
                                self.publish_selection("coalesced-event", Some(text.clone()));
                                let _ = self.events_tx.send(ResidentEvent::DisplayNotificationReady {
                                    signal: notification.kind.clone(),
                                    label: text,
                                    urgency: notification.urgency,
                                });
                            } else {
                                self.publish_selection(&format!("{} ({}, urgency {})", notification.kind, notification.from, notification.urgency), label.clone());
                                if let Some(text) = label {
                                    let _ = self.events_tx.send(ResidentEvent::DisplayNotificationReady {
                                        signal: notification.kind.clone(),
                                        label: text,
                                        urgency: notification.urgency,
                                    });
                                }
                            }
                            self.last_event = Some(Instant::now());
                        } else {
                            self.coalesced += 1;
                            self.publish_selection("coalesced", None);
                        }
                    }
                    None => return,
                },
            }
        }
    }
}

//! The minded-device aggregate.
//!
//! A device decides which keeper actions make sense from its roster
//! lifecycle and turns those verbs into typed orders. Serial ownership,
//! tasks, and event publication remain application concerns in `devices`;
//! the rules do not leak into HTTP, Workbench, or CLI presentation code.

use super::events::DeviceFacts;
use super::roster::{Individual, Lifecycle, Refusal, Roster};
use crate::catalog::Catalog;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeviceState {
    Accepted,
    #[allow(dead_code)]
    Disposed,
}

/// The stable keeper vocabulary. Every presentation receives these
/// names on the device read model and sends the same names back.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DeviceAction {
    Pause,
    Resume,
    Identify,
    Install,
    Update,
    FactoryReset,
}

impl DeviceAction {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Identify => "identify",
            Self::Install => "install",
            Self::Update => "update",
            Self::FactoryReset => "factory_reset",
        }
    }
}

/// A domain decision ready for the devices application service to enact.
pub enum DeviceOrder {
    Pause { device_id: String },
    Resume { device_id: String },
    Identify,
    Maintenance(MaintenanceOrder),
}

pub struct MaintenanceOrder {
    pub device_id: String,
    pub class: Option<String>,
    pub vid: u16,
    pub pid: u16,
    pub kind: String,
    pub faceplate: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub facts: DeviceFacts,
    pub state: DeviceState,
    pub minded_at: String,
}

impl Device {
    pub fn new(facts: DeviceFacts, minded_at: String) -> Self {
        Self {
            facts,
            state: DeviceState::Accepted,
            minded_at,
        }
    }

    pub fn device_id(&self) -> Option<&str> {
        self.facts.device_id.as_deref()
    }

    /// Actions are domain facts, not buttons inferred independently by
    /// every client. An empty list means maintenance owns the individual.
    pub fn available_actions(
        &self,
        individual: Option<&Individual>,
        in_maintenance: bool,
    ) -> Vec<DeviceAction> {
        if in_maintenance {
            return Vec::new();
        }
        let Some(individual) = individual else {
            return vec![DeviceAction::Install, DeviceAction::FactoryReset];
        };
        match individual.lifecycle {
            Lifecycle::New => {
                let primary = if currency_is_stale(individual) {
                    DeviceAction::Update
                } else {
                    DeviceAction::Install
                };
                vec![primary, DeviceAction::FactoryReset]
            }
            Lifecycle::Live => vec![
                DeviceAction::Pause,
                DeviceAction::Identify,
                DeviceAction::Update,
                DeviceAction::Install,
                DeviceAction::FactoryReset,
            ],
            Lifecycle::Paused => vec![
                DeviceAction::Resume,
                DeviceAction::Update,
                DeviceAction::Install,
                DeviceAction::FactoryReset,
            ],
            Lifecycle::Retired => Vec::new(),
        }
    }

    pub fn pause(&self, roster: &mut Roster) -> anyhow::Result<DeviceOrder> {
        let id = self.id_owned()?;
        roster
            .pause(&id)
            .map_err(|e| action_refusal(&self.facts.port, "pause", e, roster, &id))?;
        Ok(DeviceOrder::Pause { device_id: id })
    }

    pub fn resume(&self, roster: &mut Roster) -> anyhow::Result<DeviceOrder> {
        let id = self.id_owned()?;
        roster
            .resume(&id)
            .map_err(|e| action_refusal(&self.facts.port, "resume", e, roster, &id))?;
        Ok(DeviceOrder::Resume { device_id: id })
    }

    pub fn identify(&self, individual: Option<&Individual>) -> anyhow::Result<DeviceOrder> {
        if !individual.is_some_and(|i| i.lifecycle == Lifecycle::Live) {
            anyhow::bail!(
                "{}: identify is only available while the face is live",
                self.facts.port
            );
        }
        Ok(DeviceOrder::Identify)
    }

    pub fn install(
        &self,
        catalog: &Catalog,
        faceplate: Option<String>,
        individual: Option<&Individual>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        self.ensure_available(DeviceAction::Install, individual, in_maintenance)?;
        let kind = if self.facts.proto.as_deref() == Some("suzu/1") {
            "install"
        } else {
            "adopt"
        };
        self.maintenance(catalog, kind, faceplate, in_maintenance)
    }

    pub fn update(
        &self,
        catalog: &Catalog,
        faceplate: Option<String>,
        individual: Option<&Individual>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        self.ensure_available(DeviceAction::Update, individual, in_maintenance)?;
        self.maintenance(catalog, "soft", faceplate, in_maintenance)
    }

    pub fn factory_reset(
        &self,
        catalog: &Catalog,
        individual: Option<&Individual>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        self.ensure_available(DeviceAction::FactoryReset, individual, in_maintenance)?;
        self.maintenance(catalog, "factory", None, in_maintenance)
    }

    fn ensure_available(
        &self,
        action: DeviceAction,
        individual: Option<&Individual>,
        in_maintenance: bool,
    ) -> anyhow::Result<()> {
        // A suzu-speaking New face may be awaiting its currency verdict
        // while the admission and devices actors observe the same fact.
        // Update is intrinsically safe there; presentation still offers
        // it only once the stale verdict is part of the read model.
        if action == DeviceAction::Update
            && !in_maintenance
            && self.facts.proto.as_deref() == Some("suzu/1")
            && individual.is_some_and(|i| i.lifecycle == Lifecycle::New)
        {
            return Ok(());
        }
        if !self
            .available_actions(individual, in_maintenance)
            .contains(&action)
        {
            anyhow::bail!(
                "{}: {} is not available from this lifecycle",
                self.facts.port,
                action.as_str()
            );
        }
        Ok(())
    }

    fn maintenance(
        &self,
        catalog: &Catalog,
        kind: &str,
        requested_faceplate: Option<String>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        if in_maintenance {
            anyhow::bail!("{}: a maintenance saga is already running", self.facts.port);
        }
        let device_id = self.id_owned()?;
        let class = self.facts.class.clone();
        if let Some(dress) = &requested_faceplate {
            let declared = class
                .as_deref()
                .map(|c| catalog.faceplates_for_class(c))
                .unwrap_or_default();
            if !declared.iter().any(|f| &f.id == dress) {
                let vocabulary = declared
                    .iter()
                    .map(|f| format!("{:?}", f.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "unknown faceplate {dress:?} — this class declares: {}",
                    if vocabulary.is_empty() {
                        "none".to_string()
                    } else {
                        vocabulary
                    }
                );
            }
        }
        // A bare update keeps the current declared dress. The catalog
        // resolves faceplate + mount into the flattened install id.
        let faceplate = requested_faceplate.or_else(|| {
            catalog
                .dress(
                    self.facts.class.as_deref().unwrap_or_default(),
                    self.facts.faceplate.as_deref().unwrap_or_default(),
                    self.facts.mount.as_deref(),
                )
                .map(|info| info.id.clone())
        });
        Ok(DeviceOrder::Maintenance(MaintenanceOrder {
            device_id,
            class,
            vid: self.facts.vid,
            pid: self.facts.pid,
            kind: kind.to_string(),
            faceplate,
        }))
    }

    fn id_owned(&self) -> anyhow::Result<String> {
        self.device_id().map(str::to_string).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: no device_id — adopt the individual first",
                self.facts.port
            )
        })
    }
}

fn currency_is_stale(individual: &Individual) -> bool {
    individual
        .admission
        .as_ref()
        .and_then(|a| a.steps.iter().find(|s| s.name == "currency"))
        .is_some_and(|step| !step.ok)
}

fn action_refusal(
    port: &str,
    action: &str,
    refusal: Refusal,
    roster: &Roster,
    id: &str,
) -> anyhow::Error {
    let current = roster
        .individual(id)
        .map(|i| format!("{:?}", i.lifecycle).to_lowercase())
        .unwrap_or_else(|| "unknown".into());
    let reason = match refusal {
        Refusal::NotFrom(from) => format!("that move is only from {from} (this face is {current})"),
        Refusal::Unknown => "the roster holds no such individual".to_string(),
    };
    anyhow::anyhow!("{port}: cannot {action} — {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resident::events::AdmissionStep;
    use crate::resident::roster::AdmissionRecord;

    fn device(proto: Option<&str>) -> Device {
        Device::new(
            DeviceFacts {
                port: "/dev/ttyUSB0".into(),
                vid: 0x1a86,
                pid: 0x7523,
                class: Some("esp8266-oled".into()),
                family: Some("esp8266-oled".into()),
                variant: Some("oled-v2".into()),
                version: Some("1.2.1".into()),
                proto: proto.map(str::to_string),
                device_id: Some("device-1".into()),
                faceplate: Some("slate".into()),
                mount: Some("down".into()),
                legacy: false,
            },
            "now".into(),
        )
    }

    fn roster_live() -> Roster {
        let mut roster = Roster::new();
        roster
            .hold("device-1", "/dev/ttyUSB0", Some("esp8266-oled"), "now")
            .unwrap();
        roster
            .admission_result(
                "device-1",
                AdmissionRecord {
                    passed: true,
                    at: "now".into(),
                    steps: vec![],
                },
            )
            .unwrap();
        roster
    }

    #[test]
    fn the_device_owns_pause_and_resume_rules() {
        let device = device(Some("suzu/1"));
        let mut roster = roster_live();
        assert!(matches!(
            device.pause(&mut roster).unwrap(),
            DeviceOrder::Pause { .. }
        ));
        assert_eq!(
            roster.individual("device-1").unwrap().lifecycle,
            Lifecycle::Paused
        );
        assert!(device.identify(roster.individual("device-1")).is_err());
        assert!(matches!(
            device.resume(&mut roster).unwrap(),
            DeviceOrder::Resume { .. }
        ));
    }

    #[test]
    fn published_actions_follow_the_aggregate_lifecycle() {
        let device = device(Some("suzu/1"));
        let mut roster = roster_live();
        let live = device.available_actions(roster.individual("device-1"), false);
        assert_eq!(live[0], DeviceAction::Pause);
        assert!(live.contains(&DeviceAction::Identify));
        device.pause(&mut roster).unwrap();
        let paused = device.available_actions(roster.individual("device-1"), false);
        assert_eq!(paused[0], DeviceAction::Resume);
        assert!(!paused.contains(&DeviceAction::Identify));
        assert!(
            device
                .available_actions(roster.individual("device-1"), true)
                .is_empty()
        );
    }

    #[test]
    fn a_stale_new_face_offers_update_not_reinstall() {
        let device = device(Some("suzu/1"));
        let mut roster = Roster::new();
        roster
            .hold("device-1", "/dev/ttyUSB0", Some("esp8266-oled"), "now")
            .unwrap();
        roster
            .admission_result(
                "device-1",
                AdmissionRecord {
                    passed: false,
                    at: "now".into(),
                    steps: vec![AdmissionStep {
                        name: "currency".into(),
                        ok: false,
                        detail: "stale".into(),
                    }],
                },
            )
            .unwrap();
        assert_eq!(
            device.available_actions(roster.individual("device-1"), false),
            vec![DeviceAction::Update, DeviceAction::FactoryReset]
        );
    }

    #[test]
    fn install_translates_unknown_firmware_to_adoption() {
        let catalog = Catalog::load();
        let mut roster = Roster::new();
        roster
            .hold("device-1", "/dev/ttyUSB0", Some("esp8266-oled"), "now")
            .unwrap();
        let order = device(None)
            .install(&catalog, None, roster.individual("device-1"), false)
            .unwrap();
        let DeviceOrder::Maintenance(order) = order else {
            panic!("maintenance order")
        };
        assert_eq!(order.kind, "adopt");
    }
}

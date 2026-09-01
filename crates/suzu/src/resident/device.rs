//! Device lifecycle and action-policy aggregate.
//!
//! A device decides which operator actions are valid from its registry
//! lifecycle and turns those verbs into typed orders. Serial ownership,
//! tasks, and event publication remain application concerns in `devices`;
//! the rules do not leak into HTTP, Workbench, or CLI presentation code.

use super::events::DeviceFacts;
use super::registry::{RegisteredDevice, Lifecycle, Refusal, DeviceRegistry};
use crate::catalog::Catalog;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum DeviceState {
    Accepted,
    #[allow(dead_code)]
    Disposed,
}

/// The stable device-action vocabulary. Every client receives these
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
    pub kind: String,
    pub faceplate: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub facts: DeviceFacts,
    pub state: DeviceState,
    #[serde(rename = "minded_at")]
    pub tracked_at: String,
}

impl Device {
    pub fn new(facts: DeviceFacts, tracked_at: String) -> Self {
        Self {
            facts,
            state: DeviceState::Accepted,
            tracked_at,
        }
    }

    pub fn device_id(&self) -> Option<&str> {
        self.facts.device_id.as_deref()
    }

    /// Actions are domain facts, not buttons inferred independently by
    /// every client. An empty list means maintenance has exclusive access.
    pub fn available_actions(
        &self,
        registered_device: Option<&RegisteredDevice>,
        in_maintenance: bool,
    ) -> Vec<DeviceAction> {
        if in_maintenance {
            return Vec::new();
        }
        let factory = self.supports_native_factory_reset();
        let Some(registered_device) = registered_device else {
            let mut actions = vec![DeviceAction::Install];
            if factory {
                actions.push(DeviceAction::FactoryReset);
            }
            return actions;
        };
        let mut actions = match registered_device.lifecycle {
            Lifecycle::New => {
                let primary = if faceplate_is_outdated(registered_device) {
                    DeviceAction::Update
                } else {
                    DeviceAction::Install
                };
                vec![primary]
            }
            Lifecycle::Live => vec![
                DeviceAction::Pause,
                DeviceAction::Identify,
                DeviceAction::Update,
                DeviceAction::Install,
            ],
            Lifecycle::Paused => vec![
                DeviceAction::Resume,
                DeviceAction::Update,
                DeviceAction::Install,
            ],
            Lifecycle::Retired => Vec::new(),
        };
        if factory && registered_device.lifecycle != Lifecycle::Retired {
            actions.push(DeviceAction::FactoryReset);
        }
        actions
    }

    /// Offer a destructive reset only when the Resident implements the complete
    /// recovery path: RP2040 through native drive I/O, ESP8266 through the
    /// native ROM bootloader. Other classes declare no factory procedure yet.
    fn supports_native_factory_reset(&self) -> bool {
        matches!(
            self.facts.class.as_deref(),
            Some(c) if c == "waveshare-rp2040-matrix" || c.contains("esp8266")
        )
    }

    pub fn pause(&self, registry: &mut DeviceRegistry) -> anyhow::Result<DeviceOrder> {
        let id = self.id_owned()?;
        registry
            .pause(&id)
            .map_err(|e| action_refusal(&self.facts.port, "pause", e, registry, &id))?;
        Ok(DeviceOrder::Pause { device_id: id })
    }

    pub fn resume(&self, registry: &mut DeviceRegistry) -> anyhow::Result<DeviceOrder> {
        let id = self.id_owned()?;
        registry
            .resume(&id)
            .map_err(|e| action_refusal(&self.facts.port, "resume", e, registry, &id))?;
        Ok(DeviceOrder::Resume { device_id: id })
    }

    pub fn identify(&self, registered_device: Option<&RegisteredDevice>) -> anyhow::Result<DeviceOrder> {
        if !registered_device.is_some_and(|i| i.lifecycle == Lifecycle::Live) {
            anyhow::bail!(
                "{}: identify is only available while the device session is active",
                self.facts.port
            );
        }
        Ok(DeviceOrder::Identify)
    }

    pub fn install(
        &self,
        catalog: &Catalog,
        faceplate: Option<String>,
        registered_device: Option<&RegisteredDevice>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        self.ensure_available(DeviceAction::Install, registered_device, in_maintenance)?;
        let kind = if self.facts.proto.as_deref() == Some("suzu/1") {
            "install"
        } else {
            "provision"
        };
        self.maintenance(catalog, kind, faceplate, in_maintenance)
    }

    pub fn update(
        &self,
        catalog: &Catalog,
        faceplate: Option<String>,
        registered_device: Option<&RegisteredDevice>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        self.ensure_available(DeviceAction::Update, registered_device, in_maintenance)?;
        self.maintenance(catalog, "soft", faceplate, in_maintenance)
    }

    pub fn factory_reset(
        &self,
        catalog: &Catalog,
        registered_device: Option<&RegisteredDevice>,
        in_maintenance: bool,
    ) -> anyhow::Result<DeviceOrder> {
        self.ensure_available(DeviceAction::FactoryReset, registered_device, in_maintenance)?;
        self.maintenance(catalog, "factory", None, in_maintenance)
    }

    fn ensure_available(
        &self,
        action: DeviceAction,
        registered_device: Option<&RegisteredDevice>,
        in_maintenance: bool,
    ) -> anyhow::Result<()> {
        // A new suzu-speaking device may be awaiting its version result while
        // admission and device management process the same event. Updating is
        // safe in that state, but the UI only offers it after the read model
        // records an outdated faceplate.
        if action == DeviceAction::Update
            && !in_maintenance
            && self.facts.proto.as_deref() == Some("suzu/1")
            && registered_device.is_some_and(|i| i.lifecycle == Lifecycle::New)
        {
            return Ok(());
        }
        if !self
            .available_actions(registered_device, in_maintenance)
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
            anyhow::bail!("{}: a maintenance procedure is already running", self.facts.port);
        }
        // An order presupposes an identified individual.
        self.id_owned()?;
        let class = self.facts.class.clone();
        if let Some(faceplate_id) = &requested_faceplate {
            let declared = class
                .as_deref()
                .map(|c| catalog.faceplates_for_class(c))
                .unwrap_or_default();
            if !declared.iter().any(|f| &f.id == faceplate_id) {
                let vocabulary = declared
                    .iter()
                    .map(|f| format!("{:?}", f.id))
                    .collect::<Vec<_>>()
                    .join(", ");
                anyhow::bail!(
                    "unknown faceplate {faceplate_id:?} — this class declares: {}",
                    if vocabulary.is_empty() {
                        "none".to_string()
                    } else {
                        vocabulary
                    }
                );
            }
        }
        // An update without an explicit selection keeps the current faceplate. The catalog
        // resolves faceplate + mount into the flattened install id.
        let faceplate = requested_faceplate.or_else(|| {
            catalog
                .installed_faceplate(
                    self.facts.class.as_deref().unwrap_or_default(),
                    self.facts.faceplate.as_deref().unwrap_or_default(),
                    self.facts.mount.as_deref(),
                )
                .map(|info| info.id.clone())
        });
        Ok(DeviceOrder::Maintenance(MaintenanceOrder {
            kind: kind.to_string(),
            faceplate,
        }))
    }

    fn id_owned(&self) -> anyhow::Result<String> {
        self.device_id().map(str::to_string).ok_or_else(|| {
            anyhow::anyhow!(
                "{}: no device_id — provision the device first",
                self.facts.port
            )
        })
    }
}

fn faceplate_is_outdated(registered_device: &RegisteredDevice) -> bool {
    registered_device
        .admission
        .as_ref()
        .and_then(|a| a.steps.iter().find(|s| s.name == "faceplate-version"))
        .is_some_and(|step| !step.ok)
}

fn action_refusal(
    port: &str,
    action: &str,
    refusal: Refusal,
    registry: &DeviceRegistry,
    id: &str,
) -> anyhow::Error {
    let current = registry
        .registered_device(id)
        .map(|i| format!("{:?}", i.lifecycle).to_lowercase())
        .unwrap_or_else(|| "unknown".into());
    let reason = match refusal {
        Refusal::NotFrom(from) => format!("that move is only from {from} (this face is {current})"),
        Refusal::Unknown => "the registry contains no such device".to_string(),
    };
    anyhow::anyhow!("{port}: cannot {action} — {reason}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::resident::events::AdmissionStep;
    use crate::resident::registry::AdmissionRecord;

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

    fn active_registry() -> DeviceRegistry {
        let mut registry = DeviceRegistry::new();
        registry
            .register("device-1", "/dev/ttyUSB0", Some("esp8266-oled"), "now")
            .unwrap();
        registry
            .admission_result(
                "device-1",
                AdmissionRecord {
                    passed: true,
                    at: "now".into(),
                    steps: vec![],
                },
            )
            .unwrap();
        registry
    }

    #[test]
    fn pause_and_resume_follow_device_policy() {
        let device = device(Some("suzu/1"));
        let mut registry = active_registry();
        assert!(matches!(
            device.pause(&mut registry).unwrap(),
            DeviceOrder::Pause { .. }
        ));
        assert_eq!(
            registry.registered_device("device-1").unwrap().lifecycle,
            Lifecycle::Paused
        );
        assert!(device.identify(registry.registered_device("device-1")).is_err());
        assert!(matches!(
            device.resume(&mut registry).unwrap(),
            DeviceOrder::Resume { .. }
        ));
    }

    #[test]
    fn published_actions_follow_the_aggregate_lifecycle() {
        let device = device(Some("suzu/1"));
        let mut registry = active_registry();
        let live = device.available_actions(registry.registered_device("device-1"), false);
        assert_eq!(live[0], DeviceAction::Pause);
        assert!(live.contains(&DeviceAction::Identify));
        device.pause(&mut registry).unwrap();
        let paused = device.available_actions(registry.registered_device("device-1"), false);
        assert_eq!(paused[0], DeviceAction::Resume);
        assert!(!paused.contains(&DeviceAction::Identify));
        assert!(
            device
                .available_actions(registry.registered_device("device-1"), true)
                .is_empty()
        );
    }

    #[test]
    fn an_outdated_faceplate_offers_update_not_reinstall() {
        let device = device(Some("suzu/1"));
        let mut registry = DeviceRegistry::new();
        registry
            .register("device-1", "/dev/ttyUSB0", Some("esp8266-oled"), "now")
            .unwrap();
        registry
            .admission_result(
                "device-1",
                AdmissionRecord {
                    passed: false,
                    at: "now".into(),
                    steps: vec![AdmissionStep {
                        name: "faceplate-version".into(),
                        ok: false,
                        detail: "stale".into(),
                    }],
                },
            )
            .unwrap();
        assert_eq!(
            device.available_actions(registry.registered_device("device-1"), false),
            // FactoryReset rides along: the esp8266 class has a native
            // recovery path.
            vec![DeviceAction::Update, DeviceAction::FactoryReset]
        );
    }

    #[test]
    fn factory_reset_is_only_offered_with_a_native_recovery_path() {
        let registry = active_registry();
        // ESP8266 recovery runs through the native ROM bootloader.
        let esp = device(Some("suzu/1"));
        assert!(esp
            .available_actions(registry.registered_device("device-1"), false)
            .contains(&DeviceAction::FactoryReset));

        // RP2040 recovery runs through native drive I/O.
        let mut rp = device(Some("suzu/1"));
        rp.facts.class = Some("waveshare-rp2040-matrix".into());
        assert!(rp
            .available_actions(registry.registered_device("device-1"), false)
            .contains(&DeviceAction::FactoryReset));

        // A class with no native recovery path never sees the button.
        let mut td = device(Some("suzu/1"));
        td.facts.class = Some("tdisplay-esp32-ch9102".into());
        assert!(!td
            .available_actions(registry.registered_device("device-1"), false)
            .contains(&DeviceAction::FactoryReset));
    }

    #[test]
    fn install_translates_unknown_firmware_to_provisioning() {
        let catalog = Catalog::load();
        let mut registry = DeviceRegistry::new();
        registry
            .register("device-1", "/dev/ttyUSB0", Some("esp8266-oled"), "now")
            .unwrap();
        let order = device(None)
            .install(&catalog, None, registry.registered_device("device-1"), false)
            .unwrap();
        let DeviceOrder::Maintenance(order) = order else {
            panic!("maintenance order")
        };
        assert_eq!(order.kind, "provision");
    }
}

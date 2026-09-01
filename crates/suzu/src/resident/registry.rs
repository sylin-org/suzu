//! Persistent device registry and lifecycle state (ADR-0003).
//!
//! A device is **New**, **Live**, or **Paused**:
//!
//! ```text
//! new --admission passed--> live ⇄ paused
//!  |  ^-- install brings it back --^
//!  └─ retired (deliberate, final)
//! ```
//!
//! - **Live** — subscribed to host metrics, scalar updates, and notifications.
//!   Entered only after a New device passes admission.
//! - **New** — present, not on the stream. Whatever the reason (just
//!   plugged in, failed admission, unknown firmware) the available actions are
//!   same pair of tools: Install Firmware, or Factory Reset.
//! - **Paused** — streaming was disabled by the user. Admission is not
//!   withdrawn: resume re-subscribes without a re-test.
//!
//! The registry has no serial, socket, or async I/O. It consumes Resident
//! events and answers lifecycle queries. Device management reads its snapshot
//! for routing, and clients render the same snapshot.

use super::events::AdmissionStep;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Device lifecycle states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    /// Present, not on the stream. Install firmware or factory reset.
    New,
    /// Host metrics, scalar updates, and notifications are routed to the device.
    Live,
    /// Streaming is paused. Resume re-subscribes.
    Paused,
    /// Deliberately retired. Never streamed again.
    Retired,
}

/// An admission test's stored verdict.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionRecord {
    pub passed: bool,
    pub at: String,
    pub steps: Vec<AdmissionStep>,
}

/// One recorded maintenance step.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceStep {
    pub index: u32,
    pub total: u32,
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// Maintenance progress exposed to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MaintenanceState {
    pub kind: String,
    pub state: String, // running · done · failed
    pub steps: Vec<MaintenanceStep>,
}

/// Persistent state for a registered device, keyed by device ID rather than port.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisteredDevice {
    pub device_id: String,
    pub label: Option<String>,
    pub class: Option<String>,
    pub last_port: Option<String>,
    pub lifecycle: Lifecycle,
    pub since: String,
    /// The last admission verdict — `None` until the first test ran.
    pub admission: Option<AdmissionRecord>,
    /// The running (or last) maintenance progress, if any.
    pub maintenance: Option<MaintenanceState>,
}

/// Reason a lifecycle transition was refused.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No such registered device.
    Unknown,
    /// The transition makes no sense from this lifecycle.
    NotFrom(&'static str),
}

/// DeviceRegistry read model rendered by clients.
#[derive(Debug, Default, Serialize)]
pub struct DeviceRegistry {
    #[serde(rename = "individuals")]
    registered_devices: BTreeMap<String, RegisteredDevice>,
}

impl DeviceRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// A candidate appeared on `port`: it is New — present, not on the
    /// stream, whatever its firmware. Everything that follows is the
    /// user action or admission result.
    pub fn register(
        &mut self,
        device_id: &str,
        port: &str,
        class: Option<&str>,
        now: &str,
    ) -> Result<(), Refusal> {
        let registered_device = self.registered_devices.entry(device_id.to_string()).or_insert_with(
            || RegisteredDevice {
                device_id: device_id.to_string(),
                label: None,
                class: class.map(|s| s.to_string()),
                last_port: Some(port.to_string()),
                lifecycle: Lifecycle::New,
                since: now.to_string(),
                admission: None,
                maintenance: None,
            },
        );
        registered_device.lifecycle = Lifecycle::New;
        registered_device.last_port = Some(port.to_string());
        if class.is_some() {
            registered_device.class = class.map(|s| s.to_string());
        }
        Ok(())
    }

    /// The admission verdict: only a pass brings a New device Live.
    pub fn admission_result(
        &mut self,
        device_id: &str,
        record: AdmissionRecord,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        let passed = record.passed;
        ind.admission = Some(record);
        if passed && ind.lifecycle == Lifecycle::New {
            ind.lifecycle = Lifecycle::Live;
        }
        Ok(ind.lifecycle)
    }

    /// Pause a Live device stream. Resume does not repeat admission.
    pub fn pause(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle != Lifecycle::Live {
            return Err(Refusal::NotFrom("live"));
        }
        ind.lifecycle = Lifecycle::Paused;
        Ok(ind.lifecycle)
    }

    /// Resume streaming: Paused → Live, without another admission test.
    pub fn resume(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle != Lifecycle::Paused {
            return Err(Refusal::NotFrom("paused"));
        }
        ind.lifecycle = Lifecycle::Live;
        Ok(ind.lifecycle)
    }

    /// Start maintenance and remove the device from the stream. The
    /// device remains New until admission passes after maintenance.
    pub fn maintenance_started(
        &mut self,
        device_id: &str,
        kind: &str,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.maintenance.as_ref().is_some_and(|s| s.state == "running") {
            return Err(Refusal::NotFrom("maintenance is already running"));
        }
        ind.lifecycle = Lifecycle::New;
        ind.maintenance = Some(MaintenanceState {
            kind: kind.to_string(),
            state: "running".into(),
            steps: Vec::new(),
        });
        Ok(ind.lifecycle)
    }

    /// Append a maintenance step.
    pub fn maintenance_step(&mut self, device_id: &str, step: MaintenanceStep) {
        if let Some(ind) = self.registered_devices.get_mut(device_id)
            && let Some(progress) = &mut ind.maintenance {
                progress.steps.push(step);
            }
    }

    /// Finish maintenance. The device remains New until admission passes.
    pub fn maintenance_completed(
        &mut self,
        device_id: &str,
        ok: bool,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if let Some(progress) = &mut ind.maintenance {
            progress.state = if ok { "done".into() } else { "failed".into() };
        }
        ind.lifecycle = Lifecycle::New;
        Ok(ind.lifecycle)
    }

    /// The port went away: remembered, off the stream.
    pub fn departed(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle != Lifecycle::Retired {
            ind.lifecycle = Lifecycle::New;
        }
        Ok(ind.lifecycle)
    }

    /// Permanently retire a device.
    pub fn retire(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.registered_devices.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        ind.lifecycle = Lifecycle::Retired;
        Ok(ind.lifecycle)
    }

    /// The routing question the transports once asked per fan-out.
    /// (The host-state cache replaced the fan-out; the tests keep the
    /// question — it is still the lifecycle's sharpest verdict.)
    #[cfg(test)]
    pub fn is_streaming(&self, device_id: &str) -> bool {
        self.registered_devices
            .get(device_id)
            .is_some_and(|i| i.lifecycle == Lifecycle::Live)
    }

    /// Cheap snapshot, ordered by device_id.
    pub fn snapshot(&self) -> Vec<RegisteredDevice> {
        self.registered_devices.values().cloned().collect()
    }

    pub fn registered_device(&self, device_id: &str) -> Option<&RegisteredDevice> {
        self.registered_devices.get(device_id)
    }

    /// The registered device currently associated with `port`.
    pub fn by_port(&self, port: &str) -> Option<&RegisteredDevice> {
        self.registered_devices
            .values()
            .find(|i| i.last_port == Some(port.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registered(r: &mut DeviceRegistry) {
        r.register("id-1", "COM9", Some("class"), "now").unwrap();
    }

    fn pass(r: &mut DeviceRegistry) {
        r.admission_result(
            "id-1",
            AdmissionRecord { passed: true, at: "now".into(), steps: vec![] },
        )
        .unwrap();
    }

    #[test]
    fn new_devices_stream_only_after_admission_passes() {
        let mut r = DeviceRegistry::new();
        registered(&mut r);
        assert_eq!(r.registered_device("id-1").unwrap().lifecycle, Lifecycle::New);
        assert!(!r.is_streaming("id-1"));

        r.admission_result(
            "id-1",
            AdmissionRecord { passed: false, at: "now".into(), steps: vec![] },
        )
        .unwrap();
        assert!(!r.is_streaming("id-1"), "a failed admission never goes live");

        pass(&mut r);
        assert!(r.is_streaming("id-1"));
    }

    #[test]
    fn reconnected_devices_repeat_admission() {
        let mut r = DeviceRegistry::new();
        registered(&mut r);
        pass(&mut r);
        assert!(r.is_streaming("id-1"));

        // replug: back to New, retested like anyone else
        registered(&mut r);
        assert!(!r.is_streaming("id-1"));
        pass(&mut r);
        assert!(r.is_streaming("id-1"));
    }

    #[test]
    fn pause_and_resume_update_streaming_state() {
        let mut r = DeviceRegistry::new();
        registered(&mut r);
        pass(&mut r);
        assert!(r.is_streaming("id-1"));

        r.pause("id-1").unwrap();
        assert_eq!(r.registered_device("id-1").unwrap().lifecycle, Lifecycle::Paused);
        assert!(!r.is_streaming("id-1"));

        // resubscribing is not re-testing
        r.resume("id-1").unwrap();
        assert!(r.is_streaming("id-1"));
        assert!(r.registered_device("id-1").unwrap().admission.is_some());
    }

    #[test]
    fn pause_only_applies_to_live_devices() {
        let mut r = DeviceRegistry::new();
        registered(&mut r);
        assert!(matches!(r.pause("id-1"), Err(Refusal::NotFrom(_))));
    }

    #[test]
    fn maintenance_completion_requires_new_admission() {
        let mut r = DeviceRegistry::new();
        registered(&mut r);
        pass(&mut r);
        r.maintenance_started("id-1", "install").unwrap();
        assert!(!r.is_streaming("id-1"));
        r.maintenance_step("id-1", MaintenanceStep { index: 1, total: 4, name: "backup".into(), ok: true, detail: "".into() });
        r.maintenance_completed("id-1", true).unwrap();

        let ind = r.registered_device("id-1").unwrap();
        assert_eq!(ind.lifecycle, Lifecycle::New);
        assert_eq!(ind.maintenance.as_ref().unwrap().state, "done");
    }

    #[test]
    fn departure_retains_the_registered_device() {
        let mut r = DeviceRegistry::new();
        registered(&mut r);
        r.departed("id-1").unwrap();
        let ind = r.registered_device("id-1").unwrap();
        assert_eq!(ind.lifecycle, Lifecycle::New);
        assert_eq!(ind.last_port.as_deref(), Some("COM9"));
    }
}

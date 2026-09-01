//! Typed Resident events published by domains.
//!
//! Domains exchange typed commands, broadcast events, and snapshots.
//!
//! Under ADR-0004, every `/api/events` connection starts with a complete
//! snapshot. Subsequent `Devices` and `DeviceRegistry` events replace those
//! client collections; other events are deltas.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize)]
pub struct DeviceFacts {
    pub port: String,
    pub vid: u16,
    pub pid: u16,
    pub class: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub version: Option<String>,
    /// `"suzu/1"` once installed; absent until the device reports it.
    pub proto: Option<String>,
    pub device_id: Option<String>,
    /// Faceplate name reported by the device descriptor, before the mount is
    /// incorporated into the installation identifier.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faceplate: Option<String>,
    /// Faceplate mount: down | up | left | right.
    /// Absent on a single-variant faceplate or older firmware.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// True when identified only by legacy CSV identity.
    pub legacy: bool,
}

/// One tracked device serialized for clients.
/// owning domain, replaced whole in every client (ADR-0004).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceRow {
    pub port: String,
    pub class: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub version: Option<String>,
    pub proto: Option<String>,
    pub device_id: Option<String>,
    pub state: super::device::DeviceState,
    /// Actions currently allowed by the device aggregate. Workbench and CLI
    /// render this vocabulary instead of re-deriving lifecycle rules.
    #[serde(default)]
    pub actions: Vec<super::device::DeviceAction>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faceplate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mount: Option<String>,
    /// Registry lifecycle state for this device, if known.
    pub lifecycle: Option<String>,
    /// Whether the stream currently flows to this device.
    pub streaming: bool,
    /// Seconds since the device last received host data.
    pub last_data_s: Option<u64>,
}

/// One formatted journal entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalLine {
    pub ts: String,
    pub domain: String,
    pub text: String,
}

/// Resident service status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServiceFacts {
    pub name: String,
    pub version: String,
    /// In-memory pause flag, reset when the process restarts.
    pub paused: bool,
}

/// Latest device frame as a base64-encoded PNG.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FrameFacts {
    pub port: String,
    pub png: String,
}

/// Complete state sent when a client connects.
/// (ADR-0004). Everything after it on the wire is a delta.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ResidentSnapshot {
    pub service: ServiceFacts,
    pub devices: Vec<DeviceRow>,
    #[serde(rename = "roster")]
    pub registry: Vec<super::registry::RegisteredDevice>,
    pub jobs: Vec<super::jobs::Job>,
    /// The journal tail, oldest first.
    pub journal: Vec<JournalLine>,
    pub frames: Vec<FrameFacts>,
    /// Whether a client currently requests media frames.
    pub media_watched: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ResidentEvent {
    // watcher events
    DeviceSensed { port: String },
    PortBusy { port: String, reason: String },
    DeviceIdentified(DeviceFacts),
    DeviceGone { port: String },

    // device events
    #[serde(rename = "device_minded")]
    DeviceTracked { port: String, device_id: Option<String>, class: Option<String>, state: String },
    DeviceReleased { port: String, device_id: Option<String> },
    #[serde(rename = "device_homecoming")]
    DeviceReconnected { port: String, device_id: String },

    // sensor events
    #[serde(rename = "ground_changed")]
    HostMetricsChanged {
        name: String,
        uptime_s: u64,
        cpu: u8,
        mem: u8,
        disk: u8,
        /// `None` is "not measured" and displays as a dash rather than zero.
        gpu: Option<u8>,
    },
    /// High-frequency scalar sensor update.
    Pulse { axis: &'static str, value: u8 },

    // Display notification events.
    #[serde(rename = "splash_decided")]
    DisplayEventSelected { decision: String, label: Option<String> },
    /// A notification sent to device displays: the band shows the label briefly.
    #[serde(rename = "ring")]
    DisplayNotificationReady {
        /// names the display event and selects its icon when available
        signal: String,
        label: String,
        urgency: u8,
    },

    // any domain, before tripping
    Degraded { domain: &'static str, reason: String },

    // registry lifecycle events (ADR-0003)
    /// A device was added to the registry but is not yet streaming.
    #[serde(rename = "individual_held")]
    DeviceRegistered {
        device_id: String,
        port: String,
        class: Option<String>,
    },
    /// The admission test's verdict, step by step. Only a pass grants a
    /// stream subscription.
    AdmissionReport {
        device_id: String,
        port: String,
        passed: bool,
        steps: Vec<AdmissionStep>,
    },
    /// Device streaming was enabled.
    StreamAttached { device_id: String, port: String },
    /// A subscription was withdrawn (maintenance, departure, failed
    /// admission). The device enters its firmware-defined idle state.
    StreamDetached {
        device_id: String,
        port: String,
        reason: String,
    },
    /// Maintenance started for a device.
    MaintenanceStarted {
        device_id: String,
        port: String,
        kind: String,
    },
    MaintenanceStep {
        device_id: String,
        step: String,
        /// One-based step number and planned total.
        index: u32,
        total: u32,
        ok: bool,
        detail: String,
    },
    MaintenanceCompleted {
        device_id: String,
        kind: String,
        ok: bool,
    },
    /// The user permanently retired the device.
    /// The API does not yet expose this action.
    #[allow(dead_code)]
    Retired { device_id: String },

    // Long-running job events.
    Job { job: super::jobs::Job },

    // Client read-model events (ADR-0004).
    /// The devices read model, replaced whole. Published when the rows
    /// change, including each host-metrics publication.
    Devices { rows: Vec<DeviceRow> },
    /// The registry read model, replaced whole. Published after every
    /// registry mutation.
    #[serde(rename = "roster")]
    DeviceRegistry {
        #[serde(rename = "individuals")]
        registered_devices: Vec<super::registry::RegisteredDevice>,
    },
    /// The pause flag moved.
    Paused { paused: bool },
    /// Media subscription state changed.
    MediaWatched { watched: bool },
    /// Latest captured device frame (ADR-0004).
    /// Frame data is not written to the journal or text log.
    Frame { port: String, png: String },
    /// Complete state. Sent directly as the first `/api/events` frame.
    Snapshot { snapshot: ResidentSnapshot },
}

#[cfg(test)]
mod tests {
    use super::ResidentEvent;

    #[test]
    fn registry_event_keeps_the_existing_wire_names() {
        let value = serde_json::to_value(ResidentEvent::DeviceRegistry {
            registered_devices: Vec::new(),
        })
        .unwrap();
        assert_eq!(value["type"], "roster");
        assert_eq!(value["individuals"], serde_json::json!([]));
    }
}

/// Serializable admission-test result.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AdmissionStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

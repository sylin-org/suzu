//! The house events — facts, past tense, published by domains.
//!
//! The communication law: domains talk in commands (imperative, to the
//! owner), events (facts, broadcast), and cheap objects (snapshots).
//! This file owns the event vocabulary. The moments domain subscribes;
//! the logger listens; nobody reaches into anybody.

use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceFacts {
    pub port: String,
    pub vid: u16,
    pub pid: u16,
    pub class: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub version: Option<String>,
    /// `"suzu/1"` once migrated; absent on pre-suzu firmware.
    pub proto: Option<String>,
    pub device_id: Option<String>,
    /// True when identified only by legacy CSV identity.
    pub legacy: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HouseEvent {
    // watcher → the house
    DeviceSensed { port: String },
    PortBusy { port: String, reason: String },
    DeviceIdentified(DeviceFacts),
    DeviceGone { port: String },

    // devices → the house
    DeviceMinded { port: String, device_id: Option<String>, class: Option<String>, state: String },
    DeviceReleased { port: String, device_id: Option<String> },
    DeviceHomecoming { port: String, device_id: String },

    // sensor → the house
    GroundChanged {
        name: String,
        uptime_s: u64,
        cpu: u8,
        mem: u8,
        disk: u8,
        /// `None` is "not measured" — dash on the face, never zero.
        gpu: Option<u8>,
    },
    /// The pulse lane — fast, cheap, drift-or-value atoms.
    Pulse { axis: &'static str, value: u8 },

    // moments → the house
    SplashDecided { decision: String, label: Option<String> },
    /// A moment bound for faces: the band shows the label briefly.
    Ring {
        /// names the moment — and its icon, when the face has one
        signal: String,
        label: String,
        urgency: u8,
    },

    // any domain, before tripping
    Degraded { domain: &'static str, reason: String },

    // roster → the house: the device lifecycle (ADR-0003)
    /// An individual was placed in Convalescing: admitted to the roster,
    /// not yet trusted with the stream.
    IndividualHeld {
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
    /// A subscription was granted: ground, pulses and rings now route.
    StreamAttached { device_id: String, port: String },
    /// A subscription was withdrawn (maintenance, departure, failed
    /// admission). The face falls to its own honesty: idle.
    StreamDetached {
        device_id: String,
        port: String,
        reason: String,
    },
    /// The maintenance saga's spine — the workbench renders these as
    /// the step-by-step, the log keeps them as the record.
    MaintenanceStarted {
        device_id: String,
        port: String,
        kind: String,
    },
    MaintenanceStep {
        device_id: String,
        step: String,
        /// 1-based step number and the saga's planned total.
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
    /// The keeper retired the individual — deliberate, final.
    /// (The retire verb lands with the servicing engine's UI.)
    #[allow(dead_code)]
    Retired { device_id: String },
}

/// One admission-test step's shape — cheap, serializable, honest.
#[derive(Debug, Clone, Serialize)]
pub struct AdmissionStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

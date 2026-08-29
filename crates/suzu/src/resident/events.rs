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
}

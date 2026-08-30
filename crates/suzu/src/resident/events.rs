//! The house events — facts, past tense, published by domains.
//!
//! The communication law: domains talk in commands (imperative, to the
//! owner), events (facts, broadcast), and cheap objects (snapshots).
//! This file owns the event vocabulary. The moments domain subscribes;
//! the logger listens; nobody reaches into anybody.
//!
//! ADR-0004 adds the wire-side of the same law: the read models ride
//! the wire as whole-slice facts (`Devices`, `Roster`) replaced
//! wholesale by the client, and every `/api/events` connection opens
//! with one `Snapshot` — the whole house in one object. Everything
//! after is a delta. Thin per-entity patches were tried on the bench
//! and produced three timers racing one stream; whole slices and one
//! store are the repair.

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
    /// The faceplate this face wears, as its own descriptor says it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub faceplate: Option<String>,
    /// True when identified only by legacy CSV identity.
    pub legacy: bool,
}

/// One minded device, as the wire carries it — a copy, taken by the
/// owning domain, replaced whole in every client (ADR-0004).
#[derive(Debug, Clone, Serialize)]
pub struct DeviceRow {
    pub port: String,
    pub class: Option<String>,
    pub family: Option<String>,
    pub variant: Option<String>,
    pub version: Option<String>,
    pub proto: Option<String>,
    pub device_id: Option<String>,
    pub state: super::devices::DeviceState,
    /// The roster's lifecycle verdict for this individual, if known.
    pub lifecycle: Option<String>,
    /// Whether the stream currently flows to this device.
    pub streaming: bool,
    /// Seconds since the face last heard from the house.
    pub last_data_s: Option<u64>,
}

/// One journal line — what the house heard, in the house's voice.
#[derive(Debug, Clone, Serialize)]
pub struct JournalLine {
    pub ts: String,
    pub domain: String,
    pub text: String,
}

/// The service's own facts — the pill in the workbench's lampband.
#[derive(Debug, Clone, Serialize)]
pub struct ServiceFacts {
    pub name: String,
    pub version: String,
    /// The pause flag: in-memory, dies with the process.
    pub paused: bool,
}

/// A face's latest frame: PNG bytes, base64 — the media lane.
#[derive(Debug, Clone, Serialize)]
pub struct FrameFacts {
    pub port: String,
    pub png: String,
}

/// The connection-opening fact: the whole house in one object
/// (ADR-0004). Everything after it on the wire is a delta.
#[derive(Debug, Clone, Serialize)]
pub struct HouseSnapshot {
    pub service: ServiceFacts,
    pub devices: Vec<DeviceRow>,
    pub roster: Vec<super::roster::Individual>,
    pub jobs: Vec<super::jobs::Job>,
    /// The journal tail, oldest first.
    pub journal: Vec<JournalLine>,
    pub frames: Vec<FrameFacts>,
    /// Whether the media lane is watched — a window asserted it and
    /// no restart has reset it (the amendment to this ADR).
    pub media_watched: bool,
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

    // jobs — every long-running operation announces itself here
    Job { job: super::jobs::Job },

    // ── the wire vocabulary (ADR-0004) ─────────────────────────────
    /// The devices read model, replaced whole. Published when the rows
    /// change — including every ground publish, so a client's age
    /// displays breathe with the house's own cadence.
    Devices { rows: Vec<DeviceRow> },
    /// The roster read model, replaced whole. Published after every
    /// roster mutation; the lifecycle's law lives here, once.
    Roster { individuals: Vec<super::roster::Individual> },
    /// The pause flag moved.
    Paused { paused: bool },
    /// The media lane's watch flag moved (ADR-0004, the watched lane).
    MediaWatched { watched: bool },
    /// A face's latest frame — the media lane, house-cadenced (ADR-0004).
    /// Data, not news: never journaled, never announced in text.
    Frame { port: String, png: String },
    /// The whole house in one object. Never broadcast on the bus: the
    /// door writes it as the first frame of every `/api/events` stream.
    Snapshot { snapshot: HouseSnapshot },
}

/// One admission-test step's shape — cheap, serializable, honest.
#[derive(Debug, Clone, Serialize)]
pub struct AdmissionStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

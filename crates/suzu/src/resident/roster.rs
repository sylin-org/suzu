//! The roster — the fleet's individuals and their lifecycle (ADR-0003).
//!
//! The user-facing formula is the law, and it is deliberately short.
//! A device is **New**, **Live**, or **Paused**:
//!
//! ```text
//! new --admission passed--> live ⇄ paused      (the keeper's toggle)
//!  |  ^-- install brings it back --^
//!  └─ retired (deliberate, final)
//! ```
//!
//! - **Live** — subscribed to the streams (ground, pulses, rings).
//!   Granted exactly one way: a passing admission test, from New.
//! - **New** — present, not on the stream. Whatever the reason (just
//!   plugged in, failed its exam, pre-suzu firmware) the remedy is the
//!   same pair of tools: Install Firmware, or Factory Reset.
//! - **Paused** — the keeper lifted it off the stream. Trust is not
//!   withdrawn: resume re-subscribes without a re-test.
//!
//! The roster is a pure domain: no serial, no sockets, no tokio. It
//! consumes house facts and answers questions. Transport (the devices
//! domain) reads the shared snapshot to route; the log keeps the
//! events; the workbench renders the snapshot.

use super::events::AdmissionStep;
use serde::Serialize;
use std::collections::BTreeMap;

/// The lifecycle of an individual, in the keeper's own words.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    /// Present, not on the stream. Install firmware or factory reset.
    New,
    /// On the stream: ground, pulses and rings route to it.
    Live,
    /// The keeper lifted it off the stream. Resume re-subscribes.
    Paused,
    /// Deliberately retired. Never streamed again.
    Retired,
}

/// An admission test's stored verdict.
#[derive(Debug, Clone, Serialize)]
pub struct AdmissionRecord {
    pub passed: bool,
    pub at: String,
    pub steps: Vec<AdmissionStep>,
}

/// One step of a maintenance saga's journal.
#[derive(Debug, Clone, Serialize)]
pub struct SagaStep {
    pub name: String,
    pub ok: bool,
    pub detail: String,
}

/// A maintenance saga's read model.
#[derive(Debug, Clone, Serialize)]
pub struct SagaState {
    pub kind: String,
    pub state: String, // running · done · failed
    pub steps: Vec<SagaStep>,
}

/// The individual — the aggregate root. Identity survives wipes,
/// replugs, and ports; nothing here is keyed on a COM number.
#[derive(Debug, Clone, Serialize)]
pub struct Individual {
    pub device_id: String,
    pub label: Option<String>,
    pub class: Option<String>,
    pub last_port: Option<String>,
    pub lifecycle: Lifecycle,
    pub since: String,
    /// The last admission verdict — `None` until the first test ran.
    pub admission: Option<AdmissionRecord>,
    /// The running (or last) maintenance saga, if any.
    pub maintenance: Option<SagaState>,
}

/// Why a transition was refused — the roster never lies by omission.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No such individual.
    Unknown,
    /// The transition makes no sense from this lifecycle.
    NotFrom(&'static str),
}

/// The roster's read model — the truth the workbench renders.
#[derive(Debug, Default, Serialize)]
pub struct Roster {
    individuals: BTreeMap<String, Individual>,
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    /// A candidate appeared on `port`: it is New — present, not on the
    /// stream, whatever its firmware. Everything that follows is the
    /// keeper's choice or the exam's verdict.
    pub fn hold(
        &mut self,
        device_id: &str,
        port: &str,
        class: Option<&str>,
        now: &str,
    ) -> Result<(), Refusal> {
        let individual = self.individuals.entry(device_id.to_string()).or_insert_with(
            || Individual {
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
        individual.lifecycle = Lifecycle::New;
        individual.last_port = Some(port.to_string());
        if class.is_some() {
            individual.class = class.map(|s| s.to_string());
        }
        Ok(())
    }

    /// The admission verdict: only a pass brings a New device Live.
    pub fn admission_result(
        &mut self,
        device_id: &str,
        record: AdmissionRecord,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        let passed = record.passed;
        ind.admission = Some(record);
        if passed && ind.lifecycle == Lifecycle::New {
            ind.lifecycle = Lifecycle::Live;
        }
        Ok(ind.lifecycle)
    }

    /// The keeper lifted the individual off the stream. Only a Live
    /// device can pause; trust is not withdrawn, so resume needs no
    /// re-test.
    pub fn pause(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle != Lifecycle::Live {
            return Err(Refusal::NotFrom("live"));
        }
        ind.lifecycle = Lifecycle::Paused;
        Ok(ind.lifecycle)
    }

    /// The keeper put it back: Paused → Live. The one re-subscription
    /// that is not an admission — the keeper paused it, the keeper
    /// resumes it.
    pub fn resume(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle != Lifecycle::Paused {
            return Err(Refusal::NotFrom("paused"));
        }
        ind.lifecycle = Lifecycle::Live;
        Ok(ind.lifecycle)
    }

    /// A maintenance saga took ownership: the device is not on the
    /// stream while it runs. When the saga ends it is New again — the
    /// exam decides whether it ever goes Live.
    pub fn maintenance_started(
        &mut self,
        device_id: &str,
        kind: &str,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.maintenance.as_ref().is_some_and(|s| s.state == "running") {
            return Err(Refusal::NotFrom("a saga is already running"));
        }
        ind.lifecycle = Lifecycle::New;
        ind.maintenance = Some(SagaState {
            kind: kind.to_string(),
            state: "running".into(),
            steps: Vec::new(),
        });
        Ok(ind.lifecycle)
    }

    /// The saga's journal, appended live.
    pub fn maintenance_step(&mut self, device_id: &str, step: SagaStep) {
        if let Some(ind) = self.individuals.get_mut(device_id) {
            if let Some(saga) = &mut ind.maintenance {
                saga.steps.push(step);
            }
        }
    }

    /// The saga finished. Still New — admission decides the stream.
    pub fn maintenance_completed(
        &mut self,
        device_id: &str,
        ok: bool,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if let Some(saga) = &mut ind.maintenance {
            saga.state = if ok { "done".into() } else { "failed".into() };
        }
        ind.lifecycle = Lifecycle::New;
        Ok(ind.lifecycle)
    }

    /// The port went away: remembered, off the stream.
    pub fn departed(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle != Lifecycle::Retired {
            ind.lifecycle = Lifecycle::New;
        }
        Ok(ind.lifecycle)
    }

    /// The keeper retired the individual. Deliberate, final.
    pub fn retire(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        ind.lifecycle = Lifecycle::Retired;
        Ok(ind.lifecycle)
    }

    /// The routing question the transports ask per fan-out.
    pub fn is_streaming(&self, device_id: &str) -> bool {
        self.individuals
            .get(device_id)
            .is_some_and(|i| i.lifecycle == Lifecycle::Live)
    }

    /// Cheap snapshot, ordered by device_id.
    pub fn snapshot(&self) -> Vec<Individual> {
        self.individuals.values().cloned().collect()
    }

    pub fn individual(&self, device_id: &str) -> Option<&Individual> {
        self.individuals.get(device_id)
    }

    /// The individual currently on `port`, if the roster knows one.
    pub fn by_port(&self, port: &str) -> Option<&Individual> {
        self.individuals
            .values()
            .find(|i| i.last_port == Some(port.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(r: &mut Roster) {
        r.hold("id-1", "COM9", Some("class"), "now").unwrap();
    }

    fn pass(r: &mut Roster) {
        r.admission_result(
            "id-1",
            AdmissionRecord { passed: true, at: "now".into(), steps: vec![] },
        )
        .unwrap();
    }

    #[test]
    fn new_devices_are_not_live_until_the_exam_passes() {
        let mut r = Roster::new();
        held(&mut r);
        assert_eq!(r.individual("id-1").unwrap().lifecycle, Lifecycle::New);
        assert!(!r.is_streaming("id-1"));

        r.admission_result(
            "id-1",
            AdmissionRecord { passed: false, at: "now".into(), steps: vec![] },
        )
        .unwrap();
        assert!(!r.is_streaming("id-1"), "a failed exam never goes live");

        pass(&mut r);
        assert!(r.is_streaming("id-1"));
    }

    #[test]
    fn prior_trust_never_skips_the_exam() {
        let mut r = Roster::new();
        held(&mut r);
        pass(&mut r);
        assert!(r.is_streaming("id-1"));

        // replug: back to New, re-examined like anyone else
        held(&mut r);
        assert!(!r.is_streaming("id-1"));
        pass(&mut r);
        assert!(r.is_streaming("id-1"));
    }

    #[test]
    fn pause_and_resume_are_the_keepers_toggle() {
        let mut r = Roster::new();
        held(&mut r);
        pass(&mut r);
        assert!(r.is_streaming("id-1"));

        r.pause("id-1").unwrap();
        assert_eq!(r.individual("id-1").unwrap().lifecycle, Lifecycle::Paused);
        assert!(!r.is_streaming("id-1"));

        // resubscribing is not re-testing
        r.resume("id-1").unwrap();
        assert!(r.is_streaming("id-1"));
        assert!(r.individual("id-1").unwrap().admission.is_some());
    }

    #[test]
    fn pause_only_applies_to_live_devices() {
        let mut r = Roster::new();
        held(&mut r);
        assert!(matches!(r.pause("id-1"), Err(Refusal::NotFrom(_))));
    }

    #[test]
    fn maintenance_ends_new_and_the_exam_decides() {
        let mut r = Roster::new();
        held(&mut r);
        pass(&mut r);
        r.maintenance_started("id-1", "install").unwrap();
        assert!(!r.is_streaming("id-1"));
        r.maintenance_step("id-1", SagaStep { name: "backup".into(), ok: true, detail: "".into() });
        r.maintenance_completed("id-1", true).unwrap();

        let ind = r.individual("id-1").unwrap();
        assert_eq!(ind.lifecycle, Lifecycle::New);
        assert_eq!(ind.maintenance.as_ref().unwrap().state, "done");
    }

    #[test]
    fn departure_remembers_the_individual() {
        let mut r = Roster::new();
        held(&mut r);
        r.departed("id-1").unwrap();
        let ind = r.individual("id-1").unwrap();
        assert_eq!(ind.lifecycle, Lifecycle::New);
        assert_eq!(ind.last_port.as_deref(), Some("COM9"));
    }
}

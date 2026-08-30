//! The roster — the fleet's individuals and their lifecycle (ADR-0003).
//!
//! A device is an *individual* (its `device_id`), not a port. The
//! roster owns the lifecycle:
//!
//! ```text
//! Discovered → Convalescing → Streaming ⇄ UnderMaintenance → Convalescing
//!                                                                    ↘ Retired
//! ```
//!
//! The law this module enforces: **a subscription to the streams
//! (ground, pulses, rings) is granted only by `admission_result`, and
//! only a passing admission test grants one.** Nothing else — not port
//! presence, not prior trust, not a keeper's impatience — moves an
//! individual into Streaming.
//!
//! The roster is a pure domain: no serial, no sockets, no tokio. It
//! consumes house facts and answers questions. Transport (the devices
//! domain) reads the shared snapshot to route; the log keeps the
//! events; the workbench renders the snapshot.

use super::events::AdmissionStep;
use serde::Serialize;
use std::collections::BTreeMap;

/// The lifecycle of an individual.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Lifecycle {
    /// Seen, known to the roster, never yet admitted.
    Discovered,
    /// On the roster but untrusted: admission not yet passed this
    /// session. Receives nothing.
    Convalescing,
    /// Admission passed: ground, pulses and rings route to it.
    Streaming,
    /// Owned by a maintenance saga: the subscription is withdrawn.
    UnderMaintenance,
    /// Deliberately retired by the keeper. Never streamed again.
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

/// The roster's read model — the truth the workbench renders.
#[derive(Debug, Default, Serialize)]
pub struct Roster {
    individuals: BTreeMap<String, Individual>,
}

/// Why a transition was refused — the roster never lies by omission.
#[derive(Debug, PartialEq, Eq)]
pub enum Refusal {
    /// No such individual: hold_for_admission first.
    Unknown,
    /// The transition makes no sense from this lifecycle.
    NotFrom(&'static str, &'static str),
}

impl Roster {
    pub fn new() -> Self {
        Self::default()
    }

    /// A candidate appeared on `port`: held for admission. A known
    /// individual coming back (homecoming, maintenance finished)
    /// re-enters Convalescing — prior trust never skips the test.
    pub fn hold_for_admission(
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
                lifecycle: Lifecycle::Discovered,
                since: now.to_string(),
                admission: None,
                maintenance: None,
            },
        );
        individual.lifecycle = Lifecycle::Convalescing;
        individual.last_port = Some(port.to_string());
        if class.is_some() {
            individual.class = class.map(|s| s.to_string());
        }
        Ok(())
    }

    /// The admission verdict: only a pass grants the stream.
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
        if passed && ind.lifecycle == Lifecycle::Convalescing {
            ind.lifecycle = Lifecycle::Streaming;
        }
        Ok(ind.lifecycle)
    }

    /// A maintenance saga took ownership of the individual.
    pub fn maintenance_started(
        &mut self,
        device_id: &str,
        kind: &str,
    ) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        match ind.lifecycle {
            Lifecycle::UnderMaintenance | Lifecycle::Retired => {
                return Err(Refusal::NotFrom("maintenance", "already under maintenance or retired"))
            }
            _ => {}
        }
        ind.lifecycle = Lifecycle::UnderMaintenance;
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

    /// The saga finished. The individual lands in Convalescing either
    /// way: only a fresh admission test grants the stream again.
    pub fn maintenance_completed(&mut self, device_id: &str, ok: bool) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if let Some(saga) = &mut ind.maintenance {
            saga.state = if ok { "done".into() } else { "failed".into() };
        }
        ind.lifecycle = Lifecycle::Convalescing;
        Ok(ind.lifecycle)
    }

    /// The port went away: remembered, unattached. Streaming ends.
    pub fn departed(&mut self, device_id: &str) -> Result<Lifecycle, Refusal> {
        let Some(ind) = self.individuals.get_mut(device_id) else {
            return Err(Refusal::Unknown);
        };
        if ind.lifecycle == Lifecycle::Streaming || ind.lifecycle == Lifecycle::Convalescing {
            ind.lifecycle = Lifecycle::Discovered;
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
            .is_some_and(|i| i.lifecycle == Lifecycle::Streaming)
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
        self.individuals.values().find(|i| i.last_port == Some(port.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn held(roster: &mut Roster) {
        roster
            .hold_for_admission("id-1", "COM9", Some("class"), "now")
            .unwrap();
    }

    #[test]
    fn nothing_streams_without_a_passing_admission() {
        let mut r = Roster::new();
        held(&mut r);
        assert!(!r.is_streaming("id-1"), "held individuals never stream");

        let rec = |passed| AdmissionRecord {
            passed,
            at: "now".into(),
            steps: vec![AdmissionStep { name: "handshake".into(), ok: passed, detail: "".into() }],
        };
        assert_eq!(r.admission_result("id-1", rec(false)).unwrap(), Lifecycle::Convalescing);
        assert!(!r.is_streaming("id-1"), "a failed admission never streams");

        assert_eq!(r.admission_result("id-1", rec(true)).unwrap(), Lifecycle::Streaming);
        assert!(r.is_streaming("id-1"));
    }

    #[test]
    fn prior_trust_never_skips_the_test() {
        let mut r = Roster::new();
        held(&mut r);
        let pass = AdmissionRecord { passed: true, at: "now".into(), steps: vec![] };
        r.admission_result("id-1", pass.clone()).unwrap();
        assert!(r.is_streaming("id-1"));

        // The device replugs: homecoming re-enters Convalescing.
        held(&mut r);
        assert!(!r.is_streaming("id-1"), "homecoming must re-test");
        r.admission_result("id-1", pass).unwrap();
        assert!(r.is_streaming("id-1"));
    }

    #[test]
    fn maintenance_withdraws_the_stream_and_ends_in_convalescing() {
        let mut r = Roster::new();
        held(&mut r);
        r.admission_result(
            "id-1",
            AdmissionRecord { passed: true, at: "now".into(), steps: vec![] },
        )
        .unwrap();
        assert!(r.is_streaming("id-1"));

        r.maintenance_started("id-1", "soft").unwrap();
        assert!(!r.is_streaming("id-1"), "maintenance withdraws the stream");
        r.maintenance_step("id-1", SagaStep { name: "backup".into(), ok: true, detail: "".into() });
        r.maintenance_completed("id-1", true).unwrap();

        let ind = r.individual("id-1").unwrap();
        assert_eq!(ind.lifecycle, Lifecycle::Convalescing);
        assert_eq!(ind.maintenance.as_ref().unwrap().state, "done");
        assert!(!r.is_streaming("id-1"), "the stream returns only via admission");
    }

    #[test]
    fn departure_remembers_the_individual() {
        let mut r = Roster::new();
        held(&mut r);
        r.departed("id-1").unwrap();
        let ind = r.individual("id-1").unwrap();
        assert_eq!(ind.lifecycle, Lifecycle::Discovered);
        assert_eq!(ind.last_port.as_deref(), Some("COM9"), "the roster remembers");
    }

    #[test]
    fn retire_is_final() {
        let mut r = Roster::new();
        held(&mut r);
        r.retire("id-1").unwrap();
        assert!(matches!(
            r.hold_for_admission("id-1", "COM9", None, "now"),
            Ok(())
        ));
        // Retirement is overridden only by an explicit new admission hold.
        assert_eq!(r.individual("id-1").unwrap().lifecycle, Lifecycle::Convalescing);
    }
}

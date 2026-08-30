//! The job registry — every long-running operation is a job.
//!
//! A job has an id, a kind, a target, a state (`recording` · `done` ·
//! `failed` for record; `running` · `done` · `failed` for maintenance),
//! numbered progress, and a result. It is created the moment the
//! keeper asks, announced on the house wire as it changes, and served
//! from the registry to whoever asks. Nothing long-running happens
//! anywhere else; nothing blocks on a job to know how it is going.

use serde::Serialize;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast::Sender;

use super::events::HouseEvent;

/// One job, in full. `preview` carries the trail camera's latest frame
/// while a record runs — the workbench's preview reads it, so the pane
/// shows the very frames the GIF is taking.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Job {
    pub id: String,
    pub kind: String,
    pub target: String,
    pub device_id: Option<String>,
    /// `recording` | `done` | `failed` for record jobs.
    pub state: String,
    pub index: u32,
    pub total: u32,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gif: Option<String>,
    #[serde(skip)]
    pub preview: Option<Vec<u8>>,
    pub started_at: String,
}

pub type SharedJob = Arc<Mutex<Job>>;

/// The registry: id → the live job. Bounded — the oldest finished
/// jobs fall off as new ones arrive.
pub struct Jobs {
    map: Mutex<BTreeMap<String, SharedJob>>,
    order: Mutex<Vec<String>>,
    events: Sender<HouseEvent>,
}

const JOB_CAP: usize = 40;

impl Jobs {
    pub fn new(events: Sender<HouseEvent>) -> Self {
        Self {
            map: Mutex::new(BTreeMap::new()),
            order: Mutex::new(Vec::new()),
            events,
        }
    }

    /// Register a job and hand back the shared handle its worker will
    /// update through.
    pub fn create(&self, job: Job) -> SharedJob {
        let id = job.id.clone();
        let shared = Arc::new(Mutex::new(job));
        {
            let mut map = self.map.lock().expect("jobs lock");
            map.insert(id.clone(), Arc::clone(&shared));
            let mut order = self.order.lock().expect("jobs order lock");
            order.push(id.clone());
            while order.len() > JOB_CAP {
                let old = order.remove(0);
                map.remove(&old);
            }
        }
        self.announce(&shared);
        shared
    }

    /// Mutate a job through the registry; the announcement goes out
    /// after the mutation lands.
    pub fn with<F: FnOnce(&mut Job)>(&self, id: &str, f: F) {
        if let Some(shared) = self.map.lock().expect("jobs lock").get(id) {
            if let Ok(mut job) = shared.lock() {
                f(&mut job);
                let snapshot = job.clone();
                let _ = self.events.send(HouseEvent::Job { job: snapshot });
            }
        }
    }

    pub fn get(&self, id: &str) -> Option<Job> {
        self.map
            .lock()
            .expect("jobs lock")
            .get(id)
            .and_then(|j| j.lock().ok())
            .map(|j| j.clone())
    }

    /// The most recent job of a kind for a target (the media page's
    /// record state, for instance).
    pub fn latest(&self, target: &str, kind: &str) -> Option<Job> {
        let map = self.map.lock().expect("jobs lock");
        let order = self.order.lock().expect("jobs order lock");
        for id in order.iter().rev() {
            if let Some(job) = map.get(id).and_then(|j| j.lock().ok()) {
                if job.target == target && job.kind == kind {
                    return Some(job.clone());
                }
            }
        }
        None
    }

    fn announce(&self, shared: &SharedJob) {
        if let Ok(job) = shared.lock() {
            let _ = self.events.send(HouseEvent::Job { job: job.clone() });
        }
    }
}

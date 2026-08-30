from pathlib import Path
p = Path("crates/suzu/src/resident/devices.rs")
s = p.read_text(encoding="utf-8")

# 1. enum entries: RecordStart takes the shared job; RecordStatus goes away
old = """    /// The trail camera as a job: the registry holds its progress.
    RecordStart { port: String, job: Arc<Mutex<Job>>, secs: u32, fps: u32 },
    RecordStatus { port: String, reply: mpsc::Sender<Option<RecordState>> },"""
new = """    /// The trail camera as a job: the registry holds its progress.
    RecordStart { port: String, job: Arc<Mutex<Job>>, secs: u32, fps: u32 },"""
assert old in s
s = s.replace(old, new)

# 2. loop arm passes the job handle
old = """                        DevicesCmd::RecordStart { port, secs, fps, reply } => {
                            let res = self.record_start(&port, secs, fps);
                            let _ = reply.send(res).await;
                        }"""
if old in s:
    s = s.replace(old, "")
else:
    old2 = """                        DevicesCmd::RecordStart { port, secs, fps } => {
                            let res = self.record_start(&port, secs, fps);
                            let _ = reply.send(res).await;
                        }"""
    if old2 in s:
        s = s.replace(old2, "")

# 3. record_start / record_status: job-registry versions
start = s.find("    fn record_start(&mut self, port: &str, secs: u32, fps: u32) -> anyhow::Result<()> {")
end = s.find("    fn admission_retry(&self, port: &str) -> anyhow::Result<()> {")
assert start != -1 and end != -1 and end > start
new_fns = """    fn record_start(&mut self, port: &str, secs: u32, fps: u32) -> anyhow::Result<()> {
        if self.jobs.latest(port, "record").is_some_and(|j| j.state == "recording") {
            anyhow::bail!("{port}: a recording is already running");
        }
        let device_id = self
            .devices
            .get(port)
            .and_then(|d| d.device_id().map(|s| s.to_string()))
            .unwrap_or_default();
        let stamp = chrono::Utc::now().format("%Y%m%d-%H%M%S");
        let job = Job {
            id: format!("record-{port}-{stamp}"),
            kind: "record".into(),
            target: port.to_string(),
            device_id: Some(device_id),
            state: "recording".into(),
            total: (secs.clamp(1, 60) as u32) * fps as u32,
            started_at: chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
            ..Default::default()
        };
        let shared = self.jobs.create(job);
        self.send_to_session(port, SessionMsg::Record { job: Arc::clone(&shared) })
            .map_err(|e| {
                self.jobs.with(&shared.lock().expect("job").id.clone(), |j| {
                    j.state = "failed".into();
                });
                e
            })?;
        Ok(())
    }

"""
s = s[:start] + new_fns + s[end:]

# 4. DevicesCmd::RecordStart arm already updated above; now the enum entry shape
p.write_text(s, encoding="utf-8")
print("record_start/status done")

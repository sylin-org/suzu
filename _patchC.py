"""One-shot: the announcement pipeline. Deleted after use."""
from pathlib import Path

# ── events.rs: tag the enum so the wire carries typed facts ──
p = Path("crates/suzu/src/resident/events.rs")
s = p.read_text(encoding="utf-8")
old = """#[derive(Debug, Clone, Serialize)]
pub enum HouseEvent {"""
new = """#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum HouseEvent {"""
assert old in s
s = s.replace(old, new)
p.write_text(s, encoding="utf-8")
print("events tagged")

# ── mod.rs: the formatting becomes shareable; the House exposes its bus ──
p = Path("crates/suzu/src/resident/mod.rs")
s = p.read_text(encoding="utf-8")
old = """fn house_line(ev: &HouseEvent, journal: &Journal) {
    let say = |domain: &str, text: String| {
        line(domain, &text);
        journal.record(domain, &text);
    };
    match ev {"""
new = """fn house_line(ev: &HouseEvent, journal: &Journal) {
    let (domain, text) = format_house_event(ev);
    line(&domain, &text);
    journal.record(&domain, &text);
}

/// The house's facts, in the house's voice — one formatting, shared by
/// the console, the journal and the announcement wire.
pub(crate) fn format_house_event(ev: &HouseEvent) -> (&'static str, String) {
    let say = |domain: &'static str, text: String| (domain, text);
    match ev {"""
assert old in s
s = s.replace(old, new)

# every arm: say("x", format!(...)) -> say(x, format!(...))  (same shape, new return)
s = s.replace('say("watcher", format!("sensed {port}"))', 'say("watcher", format!("sensed {port}"))')

# the House door for the bus
s = s.replace("""    /// The moments door — visitors speak here.
    pub fn moments_door(&self) -> mpsc::Sender<MomentsCmd> {""",
"""    /// The announcement wire — the bus every client subscribes to.
    pub fn events_door(&self) -> broadcast::Sender<HouseEvent> {
        self.events.clone()
    }

    /// The moments door — visitors speak here.
    pub fn moments_door(&self) -> mpsc::Sender<MomentsCmd> {""")
p.write_text(s, encoding="utf-8")
print("mod.rs doors + formatter")

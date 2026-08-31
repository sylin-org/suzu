"""Re-apply the mailslot + substrate refactor to devices.rs from the git baseline."""
import subprocess

head = subprocess.run(
    ["git", "show", "HEAD:crates/suzu/src/resident/devices.rs"],
    capture_output=True, check=True).stdout.decode("utf-8")

src = head
applied = []

def rep(old, new, count=1):
    global src
    assert old in src, "NOT FOUND: " + old[:80]
    src = src.replace(old, new, count)
    applied.append(old[:40])

# 1. types before Device
types = '''/// The substrate (ADR-0006): the machine's freshest state, shared.
/// The sensor's facts land here as they land; sessions pull on their
/// own tick and send whatever is newer than the last they sent. The
/// substrate is never full and never delivered - it is only true.
#[derive(Default)]
pub struct Substrate {
    ground: Mutex<Option<(u64, Arc<MachineReport>)>>,
    pulse: Mutex<Option<(u64, String, u8)>>,
    next_gen: std::sync::atomic::AtomicU64,
}

impl Substrate {
    pub fn set_ground(&self, g: Arc<MachineReport>) {
        let ggen = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        *self.ground.lock().expect("substrate lock") = Some((ggen, g));
    }

    pub fn set_pulse(&self, axis: String, value: u8) {
        let ggen = self.next_gen.fetch_add(1, Ordering::Relaxed) + 1;
        *self.pulse.lock().expect("substrate lock") = Some((ggen, axis, value));
    }

    /// The newest ground published after `sent` - stamps `sent` on the way out.
    pub fn ground_since(&self, sent: &mut u64) -> Option<Arc<MachineReport>> {
        let cell = self.ground.lock().expect("substrate lock");
        let (ggen, g) = cell.as_ref()?;
        if *ggen > *sent {
            *sent = *ggen;
            Some(Arc::clone(g))
        } else {
            None
        }
    }

    /// The newest pulse published after `sent` - stamps `sent` on the way out.
    pub fn pulse_since(&self, sent: &mut u64) -> Option<(String, u8)> {
        let cell = self.pulse.lock().expect("substrate lock");
        let (ggen, axis, value) = cell.as_ref()?;
        if *ggen > *sent {
            *sent = *ggen;
            Some((axis.clone(), *value))
        } else {
            None
        }
    }
}

/// An ask: high-priority, sticky until the session picks it. A new ask
/// replaces the one waiting - the newest wins.
#[derive(Debug)]
pub enum Ask {
    Ring { signal: String, words: Vec<String>, urgency: u8 },
    Record { job_id: String, secs: u32, fps: u32 },
    Admission,
}

/// The face's pickup slot (ADR-0006): slap an ask and leave - quick,
/// never blocks, never full. The newest ask replaces whatever sat
/// there; the session picks it on its tick. The substrate is not
/// posted here at all: it is state the session pulls (see Substrate).
#[derive(Debug, Default)]
pub struct Mailslot {
    ask: Mutex<Option<Ask>>,
    wake: Condvar,
}

impl Mailslot {
    pub fn slap(&self, ask: Ask) {
        *self.ask.lock().expect("mailslot lock") = Some(ask);
        self.wake.notify_one();
    }

    pub fn pick(&self) -> Option<Ask> {
        self.ask.lock().expect("mailslot lock").take()
    }

    /// The tick's nap: ends early when a new ask is slapped.
    pub fn nap(&self, timeout: Duration) {
        let guard = self.ask.lock().expect("mailslot lock");
        if guard.is_some() {
            return;
        }
        let _ = self.wake.wait_timeout(guard, timeout);
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub enum DeviceState {'''
rep('#[derive(Debug, Clone, Serialize, PartialEq)]\npub enum DeviceState {', types, 1)

# 2. imports
rep('use std::sync::{mpsc as std_mpsc, Arc, Mutex};',
    'use std::sync::{Arc, Condvar, Mutex};', 1)

# 3. Device field
rep('    pub outbound: Option<std_mpsc::SyncSender<SessionMsg>>,',
    '    pub mailslot: Option<Arc<Mailslot>>,', 1)

# 4. Devices field
rep('''    media_watched: Arc<AtomicBool>,
    rows_dirty: bool,
}''', '''    media_watched: Arc<AtomicBool>,
    /// The machine's freshest state (ground + pulses) - sessions pull
    /// it on their tick (ADR-0006).
    substrate: Arc<Substrate>,
    rows_dirty: bool,
}''', 1)

# 5. ctor
rep('''            media_watched: Arc::new(AtomicBool::new(false)),
            rows_dirty: false,''', '''            media_watched: Arc::new(AtomicBool::new(false)),
            substrate: Arc::default(),
            rows_dirty: false,''', 1)
rep('''        jobs: Arc<Jobs>,
    ) -> Self {''', '''        jobs: Arc<Jobs>,
        substrate: Arc<Substrate>,
    ) -> Self {''', 1)
rep('''            roster,
            catalog,
            devices: BTreeMap::new(),''', '''            roster,
            catalog,
            substrate,
            devices: BTreeMap::new(),''', 1)

# 6. SESSION_MAILBOX const dies
rep('''const SESSION_MAILBOX: usize = 64;
''', '')

# 7. spawn_session: replace whole fn
i = src.find('    fn spawn_session(&mut self, facts: &DeviceFacts) {')
k = src.rfind('\n\n', 0, i) + 1
j = src.find('    fn close_session(&mut self, port: &str)', i)
assert i > 0 and j > i
new_spawn = '''    fn spawn_session(&mut self, facts: &DeviceFacts) {
        // It is a suzu face or it is not on the stream: boards that do
        // not speak suzu/1 stay minded and New - the remedy is install,
        // the same ceremony every face walks. No compat dialect exists.
        let suzu = facts.proto.as_deref() == Some("suzu/1");
        if !suzu {
            self.rows_dirty = true;
            return;
        }
        let slot = Arc::new(Mailslot::default());
        let thread_slot = Arc::clone(&slot);
        let (spec, zones) = self.frame_law_of(facts);
        let blinks = suzu && spec.is_some();
        // The dialect this face declared (absent or unknown: heard whole)
        let voice = facts
            .faceplate
            .as_deref()
            .zip(facts.class.as_deref())
            .and_then(|(fp, class)| self.catalog.faceplate(class, fp))
            .map(|f| f.rings.voice())
            .unwrap_or(RingVoice { qualifiers: true, text: true });
        let streaming = Arc::new(AtomicBool::new(false));
        let close = Arc::new(AtomicBool::new(false));
        let port = facts.port.clone();
        let events = self.events.clone();
        let jobs = Arc::clone(&self.jobs);
        let media_watched = Arc::clone(&self.media_watched);
        let substrate = Arc::clone(&self.substrate);
        let device_id = facts.device_id.clone();
        let class = facts.class.clone();
        let streaming2 = Arc::clone(&streaming);
        let close2 = Arc::clone(&close);
        let join = std::thread::Builder::new()
            .name(format!("session:{port}"))
            .spawn(move || {
                session_thread(
                    port, thread_slot, close2, streaming2, suzu, spec, zones,
                    events, jobs, media_watched, &substrate, voice, device_id,
                    class,
                )
            })
            .ok();
        self.sessions
            .insert(facts.port.clone(), SessionHandle { close, join });
        if let Some(device) = self.devices.get_mut(&facts.port) {
            device.mailslot = Some(Arc::clone(&slot));
            device.streaming = streaming;
            device.blinks = blinks;
        }
        self.rows_dirty = true;
    }
'''
src = src[:k] + new_spawn + '\n' + src[j:]

# 8. close_session: flag only
src = src.replace('''        let mut handle = self.sessions.remove(port)?;
        handle.close.store(true, Ordering::Relaxed);
        if let Some(outbound) = self.devices.get_mut(port) {
            if let Some(out) = outbound.outbound.take() {
                let _ = out.send(SessionMsg::Close);
            }
            outbound.streaming.store(false, Ordering::Relaxed);
        }
        self.frames.remove(port);
        handle.join.take()''', '''        let mut handle = self.sessions.remove(port)?;
        handle.close.store(true, Ordering::Relaxed);
        if let Some(device) = self.devices.get_mut(port) {
            device.streaming.store(false, Ordering::Relaxed);
        }
        self.frames.remove(port);
        handle.join.take()''', 1)

# 9. ring(): slap
src = src.replace('''            if let Some(outbound) = &device.outbound {
                let _ = outbound.try_send(SessionMsg::Out(Outbound::Ring {
                    signal: signal.to_string(),
                    words: words.clone(),
                    urgency,
                }));
            }''', '''            if let Some(slot) = &device.mailslot {
                slot.slap(Ask::Ring {
                    signal: signal.to_string(),
                    words: words.clone(),
                    urgency,
                });
            }''', 1)

# 10. mind(): slotless fresh device
src = src.replace('''                outbound: None,''', '''                mailslot: None,''')

# 11. gone()/pause(): clear the slot
src = src.replace('''            if let Some(device) = self.devices.get_mut(port) {
                device.outbound = None;
            }''', '''            if let Some(device) = self.devices.get_mut(port) {
                device.mailslot = None;
            }''', 1)
src = src.replace('''            device.outbound = None;''', '''            device.mailslot = None;''')
src = src.replace('''        if let Some(d) = self.devices.get_mut(port) {
            d.outbound = None;
        }''', '''        if let Some(d) = self.devices.get_mut(port) {
            d.mailslot = None;
        }''')

# 12. drop may_stream + publish + pulse fns
i = src.find('    /// The streaming gate: the roster\'s verdict, checked per fan-out.')
j = src.find('    /// The newest frame for a port, under the freshness bound.', i)
assert i > 0 and j > i
src = src[:i] + src[j:]

# 13. say_to: slap the ask
src = src.replace('''        self.send_to_session(
            port,
            SessionMsg::Out(Outbound::Ring {
                signal: signal.to_string(),
                words: text
                    .unwrap_or("")
                    .split_whitespace()
                    .map(|s| s.to_string())
                    .collect(),
                urgency: urgency_for(signal),
            }),
        )
    }''', '''        let Some(slot) = &device.mailslot else {
            anyhow::bail!("{port}: no live session");
        };
        slot.slap(Ask::Ring {
            signal: signal.to_string(),
            words: text
                .unwrap_or("")
                .split_whitespace()
                .map(|s| s.to_string())
                .collect(),
            urgency: urgency_for(signal),
        });
        Ok(())
    }''', 1)

# 14. send_to_session: slap, never full
src = src.replace('''    fn send_to_session(&self, port: &str, msg: SessionMsg) -> anyhow::Result<()> {
        let device = self
            .devices
            .get(port)
            .ok_or_else(|| anyhow::anyhow!("{port}: no minded device"))?;
        let outbound = device
            .outbound
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{port}: no live session — is the stream paused?"))?;
        outbound.try_send(msg).map_err(|e| match e {
            std_mpsc::TrySendError::Full(_) => {
                anyhow::anyhow!("{port}: the session mailbox is full — the face is stuck")
            }
            std_mpsc::TrySendError::Disconnected(_) => {
                anyhow::anyhow!("{port}: session died mid-request")
            }
        })
    }''', '''    fn send_to_session(&self, port: &str, ask: Ask) -> anyhow::Result<()> {
        let device = self
            .devices
            .get(port)
            .ok_or_else(|| anyhow::anyhow!("{port}: no minded device"))?;
        let slot = device
            .mailslot
            .as_ref()
            .ok_or_else(|| anyhow::anyhow!("{port}: no live session — is the stream paused?"))?;
        slot.slap(ask);
        Ok(())
    }''', 1)

# 15. record/admission ask slaps
src = src.replace('self.send_to_session(port, SessionMsg::Record { job_id: job_id.to_string(), secs, fps })',
                  'self.send_to_session(port, Ask::Record { job_id: job_id.to_string(), secs, fps })', 1)
src = src.replace('self.send_to_session(port, SessionMsg::Admission)',
                  'self.send_to_session(port, Ask::Admission)', 1)

# 16. imports
src = src.replace('use std::sync::{mpsc as std_mpsc, Arc, Mutex};',
                  'use std::sync::{Arc, Condvar, Mutex};', 1)

with open('crates/suzu/src/resident/devices.rs', 'w', encoding='utf-8', newline='') as f:
    f.write(src)

print("applied:", len(applied), "transforms")

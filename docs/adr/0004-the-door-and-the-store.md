# ADR-0004: The door and the store — how facts travel from the
# house to the screen

**Status:** Accepted · **Date:** 2026-08-30 · **Decider:** the Keeper
**Applies to:** the Resident (api, devices), the workbench shell and
its window

## Context

One bench night, three independent failures were measured, not
guessed. A single face that stopped answering wedged the devices
actor — every `Capture` waited synchronously on a session thread for
up to 10 s, the command queue backed up behind it, and every
devices-touching API handler awaited with no timeout at all:
`GET /api/status` hung past 15 s while `GET /api/log` answered, and
CLOSE_WAIT sockets piled up one per abandoned request. The workbench's
Start toggle spawned a second resident without asking whether the
house already lived — the newcomer failed to bind the door and went
on living doorless, a zombie that watched ports and journalled into
its own dead house. And the window itself was three timers racing one
stream: two 10-second polls, a 1.2-second re-pointing of two `<img>`
sources, and 150 ms deferred retries on roster events — four
directions patching the DOM, none of them the truth, each image swap
aborting an in-flight shot the resident still paid full capture price
for.

The disease underneath all three is the same: **nobody owned the
truth.** The house could block on a face; the shell could spawn a
second house; the window kept its own partial copy of the house's
state and reconciled it with timers.

## Decision

**One house, one door; the house never blocks on a face; snapshot +
stream is the whole truth; the client has one store.**

- **The door is claimed before the house is built.** `suzu serve`
  binds `127.0.0.1:7899` first — before any serial port is touched,
  before any domain is spawned. A second claimant exits loudly, with
  a reason, never doorless. The workbench probes the door before
  spawning and verifies the port actually freed after a shutdown; it
  reports the truth either way. One `suzu.exe`, one listener, or a
  visible refusal.
- **The devices actor routes; it never waits.** Waiting happens at
  the request edge, per request, under a hard timeout. A face's
  session thread owns its port and answers through a channel the
  actor never blocks on. A stuck face degrades only that face: the
  API answers in well under a second, other faces keep streaming and
  shooting, and every channel has capacity, every consumer a timeout,
  every overload a journal line instead of a silent hang.
- **Snapshot + stream is the whole truth.** Every `/api/events`
  connection opens with one `snapshot` fact — service, devices,
  roster, jobs, journal tail, latest frames — and everything after is
  a delta. The read models themselves ride the wire as cheap whole-
  slice facts (devices, roster) replaced wholesale by the client,
  because a patched partial copy is how drift starts. `/api/status`
  is deleted: there is one door, and it streams.
- **Media is a lane, not a poll.** *Amended 2026-08-30 — the lane is
  watched, not ambient; see the amendment below.* The house captures
  each live, decodable face on its own calm cadence and publishes
  frames as facts on the wire. Client cadence cannot flood the wire
  because the client commands nothing: at most one capture in flight
  per face, a house-enforced minimum interval, and concurrent readers
  share the cached result. A recording subsumes the preview — the
  frames the GIF is taking are the frames the wire carries. An
  explicit shot door serves the newest frame under a freshness bound
  and fails honestly when a face stops blinking.
- **The window keeps one store.** All client state lives in one
  in-memory store — service, stream health, devices, roster, jobs,
  journal, frames — mutated only by typed `ingest` reducers, one per
  fact kind. The wire's health (connected / reconnecting) is itself
  store state. Every view is a pure function from a store slice to
  DOM; a change re-renders the views that read the slice that changed.
  No event handler writes DOM state; no `setInterval` exists; there
  is nothing to poll because the truth arrives.
- **Reconnect replaces, never appends.** A fresh connection is
  seeded by its snapshot; the reducers replace whole slices, so a
  dropped stream can duplicate nothing.

## Consequences

**Positive.** "One face stuck" costs one pane, not the API. "Two
residents" is a refusal with a reason, not a zombie. "The window
lies" has no mechanism left: there is one store, fed by one stream,
seeded by one snapshot. Killing the resident and restarting it
rehydrates the roster and jobs without a page reload, because the
truth is re-poured, not reconciled.

**Negative / accepted costs.** Whole-slice facts are larger than thin
deltas; on a loopback wire with a handful of faces, the bytes are
noise. The journal deduplicates on `ts + domain + text`, so two
genuinely identical lines in the same second render as one —
accepted, they are indistinguishable anyway. *(Amended: the always-on
capture cost is superseded by the watched lane below.)*

**Rejected.** Client-side polling with a lock file to make it safe
(the poll is the bug; the lock is a second master by another name).
Per-event thin deltas patched into client maps (the roster's law
would live twice, and they would disagree — we ran that bench night).
A framework or a build step for the window (the store is two hundred
honest lines; the family standard is hand-rolled). Publishing frames
through per-client request/response (N windows would mean N capture
cadences — the house owns the cadence, the clients read the store).

## Amendment — the watched lane (2026-08-30)

The keeper asked the obvious economy question: why blink when nobody
looks? The always-on lane is amended into a **watched lane**:

- The window asserts `watch_media` through one action door
  (`POST /api/ui`) when Media is entered, and disasserts after a
  10-second debounce on leaving — tab switches, and the window
  hiding to the tray. The house gates its per-face blink on the flag
  and nothing else; repeats are free, and a snapshot that says
  *unwatched* to a window sitting on Media is re-asserted by that
  window, so a resident restart self-heals.
- The flag cannot outlive its client. A window that quits while
  watching can send no "off", so the house ties the lane to its own
  wire: when the last `/api/events` client disconnects, the lane
  rests regardless of the flag. Dead clients hold nothing.
- A recording is work, not a glance: its frames publish while it
  runs, watched or not — the GIF's frames are the preview.

**What it costs:** Media's first paint after an absence waits one
blink (≤ the cadence) for frames to warm; debugging the wire by hand
now starts with a `watch_media`. **What it buys:** the house blinks
only for someone's eyes, which was always the honest shape — ADR-0002
called the faces candles, not strobes.

## References

- [`docs/the-door-contract.md`](../../docs/the-door-contract.md) — the
  semantic exchange every command door speaks (adopted with the
  watched lane)
- `docs/adr/0002-the-workbench.md` — the window's framing, kept intact
- `docs/adr/0003-the-roster.md` — the lifecycle the Status view renders
- `crates/suzu/src/resident/api.rs` — the door: snapshot, deltas, bounded command shape
- `crates/suzu/src/resident/devices.rs` — the actor that routes and never waits
- `crates/workbench/ui/store.js` — the one store

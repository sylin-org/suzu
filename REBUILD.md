# REBUILD — the workbench mechanism, stripped and re-poured

You are a fresh session with no prior context. Your job: rebuild the
suzu workbench's *mechanism* — the way facts travel from the house to
the screen. The visual framing stays. The data path underneath is
condemned. Break it, strip it, pour it again.

This file is the whole brief. It was written by the outgoing session
after a live post-mortem on the bench; every failure in §3 was
reproduced and measured, not guessed.

**State at handoff.** HEAD `dfdc8ce`, working tree clean. The bench was
recovered during the post-mortem: one resident running, both faces
admitted and LIVE, paired shots returning 200 in under 0.5 s.

## 1. The project in sixty seconds

suzu gives home-server machines physical LED faces ("stones").

- `crates/suzu` — the CLI. `suzu serve` runs the **Resident**: a
  headless daemon that owns the serial ports, translates a ground of
  machine facts into light, and announces everything on the **house
  wire** — a loopback HTTP API on `127.0.0.1:7899` whose `/api/events`
  is a server-sent-event stream of typed facts (`HouseEvent`).
- `crates/workbench` — a Tauri 2 desktop app: the keeper's window onto
  the house. Four views: Status, Log, Media, About.
- `firmware/` — the faces themselves (CircuitPython on RP2040-Matrix,
  MicroPython on ESP8266 OLED). **Out of scope. Do not touch.**
- Domain law lives in `docs/adr/0001..0003`, `docs/the-face-contract.md`,
  `docs/wire-protocol.md`.

The roster lifecycle (ADR-0003): New → admission exam → Live ⇄ Paused
→ Retired. Only the exam grants Live; the keeper pauses and resumes;
prior trust never skips the exam.

## 2. Read these first, in this order

1. `docs/adr/0003-the-roster.md` — the lifecycle you must respect
2. `docs/adr/0002-the-workbench.md` — the framing you must keep
3. `crates/suzu/src/resident/events.rs` — `HouseEvent`, the fact vocabulary
4. `crates/suzu/src/resident/mod.rs` and `api.rs` — the house and its door
5. `crates/suzu/src/resident/devices.rs` — the domain you must un-jam
6. `crates/workbench/src/main.rs` — the shell (spawn/stop, SSE bridge)
7. `crates/workbench/ui/app.js` — the condemned patchwork; read it to
   know what *not* to rebuild

## 3. What is broken — measured on the live bench

Three failures, independent, compounding:

**W1 — the devices conveyor jams and takes the house's voice with it.**
The devices actor processes every command serially, and `Capture`
*waits synchronously* on the owning session thread — up to 10 s
(`devices.rs` `capture()` → `SessionMsg::Capture`), while the session's
own shot handshake waits up to 8 s on the wire (`shot.rs`
`capture_on`). If one face stops answering, every queued capture burns
its full timeout, the command channel (capacity 64) backs up, and every
devices-touching API handler awaits **with no timeout at all**
(`api.rs` `status()`: `rx.recv().await`). Measured: with one face
unhappy, `GET /api/log` (which never touches devices) answered 200
while `GET /api/status` hung past 15 s, and the listener piled up
CLOSE_WAIT sockets — one per abandoned request. One stuck face browns
out the entire API.

**W2 — two masters.** The workbench's Start toggle spawns a resident
without asking whether one already lives. A second `suzu.exe` fails to
bind `7899` and lives on doorless — a zombie that still watches ports
and journals into its own dead house. Measured: two `suzu.exe` PIDs at
once, exactly one owning the listener. Stop can never clean the zombie:
it only POSTs the shutdown door (owned by the *other* process) or kills
its own child.

**W3 — the client is three timers racing one stream.** `app.js` polls
`/api/status` every 10 s *plus* 150 ms deferred retries on roster
events, polls `/api/log` every 10 s, and re-points both media
`<img src>`s every 1.2 s with fresh cache-busters. State is patched
into the DOM from all four directions; the connection itself is not
state. Each img swap aborts an in-flight GET while the resident still
pays full capture price. Symptoms this produces: status flipping
running/stopped while the log flows, views that stall on tab switches,
duplicate journal rows after SSE reconnects (the shell re-seeds
`journal.tail(30)` on every connect and the UI *prepends* them).

## 4. The laws of the rebuild

1. **One house, one door.** The resident binds `7899` *before* it
   touches any serial port; a second claimant exits loudly with a
   reason. Start probes before spawning; Stop shuts down whichever
   process owns the door and verifies the port freed.
2. **The house never blocks on a face.** The devices actor routes; it
   does not wait. Waiting happens at the request edge, per request,
   with a hard timeout. A stuck face degrades only that face: status
   answers in < 1 s; other faces keep streaming and shooting.
3. **Snapshot + stream is the whole truth.** Every `/api/events`
   connection begins with one `snapshot` fact — full roster, devices,
   jobs, journal tail, service state. Everything after is a delta.
   `/api/status` is deleted (or demoted to a curl-only debug door).
   No client polling exists anywhere.
4. **The client has one store.** In-memory collections — service,
   devices, roster, jobs, journal, media — mutated only by typed
   `ingest(fact)` reducers, one per fact kind. Stream health
   (connected / reconnecting / down) is itself store state.
5. **Views read the store.** Each view is a pure function from a store
   slice to DOM; a store change re-renders the views that read it. No
   event handler ever touches the DOM directly. Zero `setInterval` in
   the UI.
6. **Captures coalesce.** At most one in-flight capture per face, and
   a per-face minimum interval enforced house-side, so no client
   cadence can flood the wire. Concurrent shot requests share the
   result. Media reads frames from the store; it never commands the
   house directly.
7. **Every wait is bounded.** Channels have capacity; consumers have
   timeouts. A full channel sheds load with a journal line, never a
   silent hang.

## 5. The target shape

**Resident** (`crates/suzu/src/resident/`):
- `api.rs`: SSE with a leading `snapshot`; every command door (pause,
  resume, admission retry, record, capture-save, maintenance, say,
  shutdown) follows one shape: send command → await reply **with
  timeout** → honest error on timeout.
- `devices.rs`: the loop only routes. Capture / record / admission ride
  the session thread; the session answers through a oneshot the loop
  never blocks on.
- Single-instance: bind first, then mind ports; bind failure exits
  with a reason the workbench can show.
- `events.rs` gains a `Snapshot` fact. Design it; don't shoehorn it.

**Client** (`crates/workbench/ui/`):
- `store.js` — the only mutable state: `ingest(fact)`, typed reducers,
  change notification, stream-health tracking.
- `feed.js` — Tauri `house` events → `store.ingest`; a reconnect
  *replaces* snapshot-seeded collections, never appends.
- one render function per view — `render(storeSlice)`.
- `index.html` / `styles.css` keep the framing. CSP is
  `style-src 'self'` — zero inline styles, classes only. No build
  step, no framework: hand-rolled, small, honest.

**Shell** (`crates/workbench/src/main.rs`):
- Start: probe the door first; refuse loudly if the house already lives.
- Stop: shutdown door, then verify the port actually freed; report the
  truth either way.
- SSE bridge keeps its spirit; reconnects must not duplicate rows.

## 6. Out of scope

- `firmware/**` — the faces are settled (ADR-0001).
- The wire protocol, admission semantics, roster lifecycle (ADR-0003).
- The maintenance sagas' contents (numbered steps stay).
- The visual design: layout, classes, the About card, the media tiles.

## 7. Definition of done — bench-verified, not assumed

- [ ] One `suzu.exe`; `netstat` shows exactly one listener on 7899.
- [ ] Start while the house lives → refused with a visible reason; no
      second process appears.
- [ ] Kill the resident from Task Manager, no mercy: the workbench
      shows *down* within the SSE timeout and nothing hangs. Restart
      it: the pill returns to *running*, roster and jobs rehydrate
      from the snapshot, no page reload.
- [ ] Loop Status → Media → Log → About twenty times: no stall, memory
      flat, no duplicated log rows.
- [ ] With one face deliberately stuck: status answers < 1 s; the
      other face keeps streaming and shooting; the stuck face's shots
      fail honestly after one bounded timeout.
- [ ] Media tiles refresh at a calm, house-enforced cadence.
- [ ] `record` 4 s produces a GIF job end-to-end, reported via Job
      facts alone.
- [ ] `cargo clippy --workspace` clean; the workbench builds; grep
      proves zero `setInterval` and zero inline styles under `ui/`.
- [ ] ADR-0004 written **before** the code: the door law and the client
      state model, in the voice of 0001–0003.

## 8. The bench

- Windows, Git Bash. Repo: this directory.
- Faces: `COM12` = ESP8266 OLED (`esp8266-oled-v2-class`), `COM16` =
  Waveshare RP2040-Matrix (5×5, RGB-wired). Both admitted and LIVE
  when the house is healthy.
- Run the house: `./target/debug/suzu.exe serve` (logs append to
  `serve.log` / `serve.err.log`) — or `cargo run -p suzu -- serve`.
- Run the window: `./target/debug/suzu-workbench.exe` — or
  `cargo tauri dev` inside `crates/workbench`.
- Probe doors by hand: `curl -m 5 http://127.0.0.1:7899/api/...`;
  watch the wire: `curl -N http://127.0.0.1:7899/api/events`.
- Serial law you must not relearn: CircuitPython's console is
  DTR-gated (no DTR → a deaf face); UART RX FIFOs overrun bursts
  (dribble writes: 16 bytes / 4 ms); pulses flow at 5 Hz and every
  consumer must drain. All already handled in `shot.rs` / `devices.rs`
  — keep their functions, re-pour their plumbing.

## 9. Ground rules

- Commit small and often, messages in the house voice
  (`feat(workbench): ...`).
- Delete dead code. No commented-out corpses, no `if false { ... }` —
  there is one fossil of exactly that kind in `shot()` today.
- When a symptom appears, find the state model that permits it and fix
  the model. A fix that adds a timer, a flag, or a special case is a
  regression, not a repair.
- The Keeper's naming is law: faces, stones, the house, the door, the
  wire, admission, the roster, moments, rings.

# Harvest analysis — the Zen Garden PoC companion surface

*What the PoC proved, how it evolved, and what suzu inherits from it.*

This document preserves the analysis of the pre-existing, working companion
code in `../zen-garden/src/poc/` — the harvest that suzu is initially based
on. Sources read in full:

- **Code**: `companion-sdk/`, `companion-usb/`, `cricket/`, `firefly/`,
  `moss/src/infra/companions.rs`, `moss/src/api/v1/companions.rs`,
  `rake/src/commands/hey.rs`, `common/src/` (companion + manifest types)
- **ADRs**: `docs/poc/decisions/COMPANION-0001` … `COMPANION-0018`
- **Guides**: `docs/poc/guides/companion-{overview,development}.md`,
  `cricket-tune-authoring.md`
- **Philosophy**: `docs/poc/philosophy/{humanist-infrastructure,
  metaphor-as-architecture,joy-in-infrastructure}.md`
- **Inventory**: `docs/v1/inventory/clients-companions.yaml`,
  `docs/v1/inventory/live-poc-harvest-2026-08-28/`, ADR-0006 (v1)

---

## 1 · The philosophy

The companion system rests on a few load-bearing convictions, stated in ADRs
and enacted consistently in code:

1. **Delight is load-bearing, not decoration.** The firefly runs an *ambient
   baseline* — warm-white firefly sprites wandering a 5×5 grid at 30 fps with
   ease-in-out fades — and events only *temporarily override* it (a
   tended-sparkle, an amber pulse when a stone withers), after which the
   baseline resumes. Cricket routes sounds through four named channels
   (foreground / midground / ambient / background) with per-event
   `debounce_ms` so the house is never nagged. The tune-authoring guide's
   design wisdom: "leave silence — not every event needs a sound," no melodic
   content in loops, foreground loudest, background subliminal.

2. **Companions are guests at the edge (B7).** Each companion process owns
   its own devices and lifecycle absolutely. The host does exactly four
   things: sends events, assigns a port, proxies commands, tracks liveness.
   It never reaches into a companion's internals. "A companion that isn't
   running is a companion that's resting."

3. **The contract is declared, not inferred.** Every companion publishes a
   command manifest at startup (`--dump-commands`). Everything else derives
   from that one declaration: the host's registry, `hey firefly?` help, the
   proxy endpoints. The CLI is deliberately a *thin* pass-through — raw args
   forwarded untouched; the companion owns its grammar and validation.

4. **Everything is an event, even commands.** Inside a companion there is one
   bus (the Pulse) and one envelope (GUIDv7 id, UTC timestamp, namespaced
   kind, type-erased typed payload). An incoming HTTP command becomes a
   `CommandInvocation` event; adapters answer with correlated `CommandResult`
   events; the transport aggregates results (zero → "no handler" error, one →
   echo, many → prefixed join) into the HTTP response.

5. **Events for deltas, reads for state.** Adapters hydrate from the host's
   REST API at spawn, then consume live deltas over SSE. The earlier
   client-side CQRS projection was deliberately deleted (COMPANION-0014).

6. **Paranoia where it's load-bearing.** The 4-second identity-handshake
   deadline is a `const {}` compile-time assert with the latency budget
   documented in the assert message (2.5 s ESP boot + 0.2 s identity emit +
   1.3 s USB hiccups). "A device that cannot answer in 4 seconds is not a
   Suzu companion — it is a serial port that happens to be attached."

## 2 · The devices it supported

**Firefly (visual), 24 devices live-minted — four firmware variants across
three hardware families:**

| Family | Chip / display | Identity | Serial vocabulary |
|---|---|---|---|
| RP2040-Matrix | Waveshare 5×5 RGB | `variant: "matrix"` | `P,x,y,r,g,b`, `F,r,g,b`, `C`, `B,percent`, `A,name` (rainbow/pulse/chase/sparkle), `S`, `T,state`, `I` |
| OLED v1 | ESP8266, 128×64 SSD1306 | `variant: "oled"`, no `dashboard` capability | stone name (`S`), health (`H`), metrics (`M,cpu,mem,uptime`), WIPE-IN/OUT |
| OLED v2 | ESP8266, same panel | `variant: "oled"` **+** `dashboard` capability | one packed frame: `D,cpu,mem,disk,uptime,offerings,stones,net_bps,seed_bank` |
| T-Display | ESP32, 135×240 ST7789 | `variant: "tdisplay"` | JSON push `J,{…}` plus discrete verbs: load, health, `+,name,h\|w` / `-,name`, tended, seed-bank `SD`/`SR` |

Device identity is a GUIDv7 minted during a provisioning ritual (roster
persisted write-then-rename). The v1-vs-v2 OLED distinction is resolved by
*capability*, not variant name. Firmware is **not** uniformly v1.0.0 — OLEDs
are 0.2.0.

**Cricket (audio):** any speaker via system audio (declares `libasound` as a
`SystemDependency`). Its soul is the **tune** — YAML mapping event kind →
`{resource, channel, debounce_ms, looping, volume}`, with a `fallback`
sample. One embedded tune (`zen-tech`, CC0 mp3s) ships in the binary;
filesystem tunes overlay embedded ones by name. It has an offline `test`
subcommand — a keyboard-driven rehearsal mode that plays events *without a
stone*, which is exactly the right developer ergonomics for a hardware
project.

Both speak line-delimited text/JSON over USB serial at 115200 baud.

## 3 · How it supported them

### The device path (plug-in → expression)

1. **Discovery** — a `Monitor` trait: `UdevMonitor` (Linux/glibc, netlink +
   mio, emits an initial enumeration snapshot so boot-time devices aren't
   missed) and `PollMonitor` (2 s diff of `available_ports()`; the fallback
   for musl/Windows/macOS). Devices identified as `usb:{serial}:{vid}:{pid}`
   with a syspath/port fallback.
2. **Registry** — the canonical owner of open devices. On Added it opens the
   port (then sleeps 2.5 s for the ESP auto-reset boot) and announces
   `Appeared`. On Removed it disposes and announces `Disappeared`.
   Crucially, **a rejected device is never abandoned**: a re-probe sweep
   re-opens rejected-but-present devices on backoff (first retry 5 s,
   doubling to a 60 s cap).
3. **Per-device driver** — one blocking thread per port: a write queue with
   per-write acks, a 20 ms read poll, complete lines broadcast to
   subscribers, state machine `New → Evaluating → Accepted{kind} |
   Rejected{reason} → Disposed`. Sustained EOF (100 zero reads — a dangling
   Linux fd) self-disposes.
4. **Identity handshake** — the probe subscribes to the line broadcast
   *before* writing `I\n` (avoiding a TOCTOU race on the reply), then awaits
   JSON within 4 s. Tolerates `OK,`- and `* HELLO,`-prefixed frames, and
   recognizes *legacy CSV identities* so old firmware gets "re-run
   NewFirefly to update the board" instead of a misleading timeout.
5. **Classification → adapter** — variant + capabilities select one of four
   adapters, spawned through the supervisor as an "external" adapter (exempt
   from discovery-tick reaping; reaped explicitly on detach).

### The runtime path (events → expression)

- `Companion` builder wires an `SseTransport` (host presence stream;
  reconnect backoff 1→2→4→8→16→32 s; unknown wire kinds log-and-skip; a
  wire→canonical kind translation table is the anti-corruption boundary) and
  a `CommandTransport` (loopback HTTP: `POST /command`, `POST /shutdown`,
  `GET /health`).
- A Pulse bus with a **50 ms coalescing flush** sits between transports and
  adapters — high-frequency state events are declared `COALESCING` at the
  payload-type level so bursts collapse to the latest value.
- The adapter supervisor ticks discovery every 5 s, dedupes by adapter id,
  reaps absent adapters after a 2 s grace window, filters Pulse events
  per-adapter by declared kind subscriptions (mpsc depth 64 for
  backpressure), and publishes exit events (Panicked/Reaped/SelfExit).
- **The expression layer** is where the philosophy lives. The matrix adapter
  maintains `Animation` state — hydrated from the host's presence snapshot at
  spawn, then updated by events — with transient `Override`s that preempt the
  baseline and expire. Cricket's singleton adapter looks up
  `tune_key(event.kind)` in the active tune, honors per-event debounce, and
  plays on the mapped channel; `off` stops all channels so nothing loops
  across a restart.

### The host side

At boot the host scans `{data_dir}/companions/*/`, probes each with
`--dump-commands` (5 s timeout), assigns a loopback port from a persisted
`PortLedger` (PoC pool 7187–7199; ADR-0006 reserves **7286–7295** for v1),
and keeps an enable/disable ledger (user intent survives restarts). After a
host restart it *adopts* companions that survived (`kill_on_drop(false)`) by
probing `GET /health` on their assigned ports (500 ms timeout); liveness is
an `alive` flag — PID is bookkeeping only. Commands flow
`rake hey tell firefly pixel 2 2 ff0000` → host proxy (auto-starts a stopped
companion; 5 s timeout) → companion loopback `/command` → Pulse → adapter →
correlated result back out. `hey tell firefly all …` fans out topology-wide,
signed, in parallel, best-effort.

## 4 · The historical arc (from the ADRs)

### Act I — the organic era (before 2026-04-13)

Firefly and cricket each wired their own SSE consumer directly to device
I/O. COMPANION-0001's context is a catalog of what that produced:
`PresenceSnapshot` defined privately in firefly while cricket matched raw
strings (silent-breakage risk); ~33 scattered `device_type ==` dispatch
sites; no event bus (no dedup, coalescing, backpressure); zero integration
tests; and the original sin — `FireflyConnection`, a shared mutex around the
serial port, where one slow I/O call could wedge the whole pipeline (a real
replug deadlock). Commands flowed through a structurally unrelated parallel
path (`CommandHandler` trait + HTTP server). The early guides still document
this era — `companion-development.md` teaches the deleted
`CommandHandler`/`CompanionRuntime` API, and `cricket-tune-authoring.md`
shows a time when adding a tune meant editing a Rust enum and submitting a
PR. Doc-vs-code drift is itself a tracked defect class in this project.

### Act II — the epic (COMPANION-0001, two days, ten "books")

2026-04-13→14: the whole segment rebuilt under a discipline worth copying:

- **Pattern spec first, then books** in dependency order (envelope → Pulse →
  transports → domain types → Garden → adapters → Companion → rebuild →
  integration tests → epilogue), each landing green on `dev`.
- **The dual-prototype gate**: before freezing the `Adapter` trait,
  throwaway prototypes for the two extremes — matrix (complex hardware) and
  audio (simple singleton). The `DynPayload` two-trait shape (Rust
  associated consts break object safety) came out of this gate.
- **Break-and-rebuild over migrate-in-place**: ~6.5k LOC is small enough
  that replacing beats strangling. A scaffolding tracker gave every
  temporary shim an ID and a removal trigger, with a CI check — closed with
  zero active entries.
- **The Discovery Mandate**: "the plan is a hypothesis… if the code teaches
  you something the plan didn't anticipate, stop and change the plan, the
  code, or both." Hardware availability, not the ADR's recommended order,
  actually drove the rebuild sequence.
- **Success criteria verified with grep evidence**, plus an honest
  postmortem. Result: "adapter count is data, not code."

### Act III — reality fights back (each ADR = a production failure answered)

1. **COMPANION-0012 (Device Bus)** — within a week of the epic closing,
   per-factory discovery broke: three factories each probed the same ports
   every 5 s tick, each open triggering an ESP32 auto-reset; once one
   factory's adapter held the port, the others got "access denied" and
   reap/respawn churn began. Hotfix (`claimed` caches) masked the gap:
   *discovery was per-adapter when it should be system-wide*. The fix's
   shape — one bus owns enumeration, identity protocols parse descriptors,
   specificity-ordered predicates claim sequentially, per-port backoff,
   explicit telemetry for `unprovisioned`/`unclaimed`/`foreign` devices —
   is the intellectual ancestor of suzu's identity handshake.

2. **COMPANION-0015 (stale fds)** — a silent unplug/replug on the same node
   name left the adapter holding a dead kernel fd, writing into the void
   with no detach event ever firing. Fix: a conservative dual-gate
   predicate — self-exit only when **5 consecutive failures AND 15 s since
   last successful I/O** — explicitly avoiding the earlier
   false-exit-on-slow-frame bug. `SelfExit` triggers a fresh identity dance.

3. **COMPANION-0016 (PID ledger is the wrong primitive)** — after a reboot
   the host "adopted a corpse": PID 1373 from yesterday was mongod today,
   so firefly never started. The correction is a principle, not a patch:
   *liveness is `GET /health` on the assigned port; the port is the
   identity; PID is bookkeeping only.*

4. **COMPANION-0018 (Three-Domain Architecture, the current law)** — the
   deepest lesson. Every bus iteration (0012, 0015, and a never-shipped
   0017 rewrite) accreted fixes onto one monolithic object entangling four
   concerns: OS discovery, identity determination, adapter lifecycle,
   byte-level I/O. Each spot-fix treated a symptom of the same mistake.
   The final answer split into three bounded contexts — `usb_devices`
   (what does the OS report?), the companion (is this device ours, and
   which kind?), `adapters` (drive it) — governed by the **Law of
   Instances**: pass instances, never ids; each layer speaks only its own
   vocabulary (`device.send` is USB vocabulary, `firefly.oled_health` is
   firefly vocabulary); reach-through is legitimate when the vocabulary
   fits; references are permanent and teardown propagates through state
   transitions. The `companion-usb` code *is* this ADR made flesh.

5. **COMPANION-0014 (the host IS the read model)** — the Book V client-side
   CQRS projection (`Garden` aggregate) was deleted. SSE doesn't replay, so
   an adapter spawned after the initial snapshot never saw it — the OLED v2
   sat on its boot placeholder. Derived fields like `uptime_seconds` coupled
   client state to event arrival order. Correction: one read path (HTTP at
   spawn), one delta path (SSE), no client-side projection, ~800 lines
   deleted.

## 5 · The philosophy that binds it

- **Humanist infrastructure** is the "why": comprehensibility over
  capability, ownership over rental, permission to be small, "failure that
  explains — weather, not error codes," joy as functional rather than
  decorative. Reliability is table stakes; humanity is what you build on top.
- **Metaphor-as-architecture** is a real design constraint: "when you add
  something to the garden, you must find its name in the garden… if you
  cannot find the garden-word for what you're building, perhaps you're
  building the wrong thing." This is the direct lineage of suzu: the
  companion ecosystem gets the garden-word for "the bell that lets the
  household hear the garden" — 鈴 — with **vesper** (evening bell) as the
  companions' own rake.
- **Joy-in-infrastructure** sets the delight budget's guardrails:
  *physicality over theater*. Never fake delays; show work as it happens;
  progressive disclosure is a teaching tool. Anti-patterns: cuteness that
  obscures errors, forced personality without opt-out, randomness in
  critical paths.

## 6 · Known drift and limitations (the "too limited in scope" part)

From the live-fleet inventory (`clients-companions.yaml`) and the live
harvest captures:

- **Event vocabulary was presence-shaped, not story-shaped.** The PoC
  consumed `stone.load.updated`, `presence.snapshot`, `storage.connected` —
  dashboard telemetry. ADR-0006 defines the v1 *heal-moment vocabulary*
  instead (`stone-seen/goodbye/expired`, `offering-planted/rested/woke/
  uprooted`, `capture-committed`, `replanted`, `health-degraded/healed`).
  `CONTRACT.md` maps these to suzu kinds.
- **Declared-but-unenforced policies**: `DeliveryPolicy::LatestEvery`/
  `Debounced` behaved as `All` (cricket hand-rolled debouncing);
  `persisted_state` was a stub.
- **Two competing enabled-flag formats** (`sse_enabled` vs `enabled`),
  bridged ad-hoc by cricket.
- **Registered-but-unreachable**: firefly live commands 504'd despite
  registration — adopt/liveness reconcile never re-verified command
  reachability.
- **Port exhaustion was a hard failure** in the PoC; ADR-0006 demands
  "loud posture degradation, never a crash."
- **Envelope drift**: companions answered bare `CommandResponse` while every
  other host endpoint wrapped in `ApiResponse` — a live bug class (bit
  twice: `election start`, `rake api`) that v1's one-envelope law kills
  structurally.
- **companion-usb was generic in name, firefly-only in fact**; the identity
  `family` field is the seam where other companions plug in.
- **Rust-trait coupling**: the SDK was a Rust crate, so a Python companion
  wasn't actually practical. ADR-0006's "the SDK is a convenience, not a
  requirement" is the correction — and CONTRACT.md's five transports (SSE,
  vesper CLI, web API, MCP, stdio) generalize what the PoC proved with two.

## 7 · What this history means for suzu

1. **The PoC's final architecture is already suzu-shaped.** After 0018, the
   code stopped being firefly's private plumbing and became three clean
   domains that suzu's contract generalizes: the identity handshake is
   `FireflyProbe` with the ecosystem widened from `family == "firefly"` to
   `proto == "suzu/1"`; the adapter profile/subscription system is the
   per-companion expression layer; the host registry/port-ledger/
   health-probe machinery is the host contract.

2. **Every good mechanism in CONTRACT.md is a scar.** The 4-second
   compile-time-asserted handshake exists because of ESP boot races and
   triple-probe storms; liveness-by-port exists because of PID reuse; the
   anti-corruption translation table exists because of wire-kind drift; the
   one-envelope law exists because the bare-vs-wrapped bug class bit twice
   live; state-over-HTTP-plus-deltas-over-SSE exists because SSE doesn't
   replay.

3. **Keep the methodology, not just the code.** The epic shows what
   design-first looked like when it worked: pattern spec first,
   prototype-gated trait freezes, scaffolds with removal triggers,
   green-per-chapter shipping, grep-verified success criteria, honest
   postmortems. Expect suzu's own "device-bus moment" in the first week of
   real validation, and leave room for the Discovery Mandate.

4. **Doc-code drift is the failure mode to institutionalize against.** The
   PoC's guides drifted badly (deleted APIs, superseded shapes, Rust-enum
   tunes). Suzu's docs *are* the product — the contract is the law — so
   drift there would be worse than cosmetic. The live-capture discipline
   (`live-poc-harvest-2026-08-28/`) is the countermeasure worth adopting
   from day one.

---

*Analysis date: 2026-08-28. All paths relative to the zen-garden repo root
(`../zen-garden/`) unless marked otherwise.*

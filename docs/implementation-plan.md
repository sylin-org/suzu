# Implementation plan

*From ideation to code: what exists, what gets borrowed, what gets built,
and in what order.*

Plan date: 2026-08-28. Rust, OS-agnostic. Under
[`the-model.md`](the-model.md) and [`wire-protocol.md`](wire-protocol.md).

---

## 1 · Verdict: yes, with named gaps

Every hard mechanism in suzu already has a live-proven reference
implementation. What remains is assembly, adaptation, and the *mixer* —
which is host-side, where firepower and iteration speed are abundant.

**Borrowed (proven in the garden):**

| Mechanism | Reference | Disposition |
|---|---|---|
| Event bus: dedup, coalescing (50 ms flush), fan-out, metrics | companion-sdk `garden/pulse.rs` | port, becomes the host's slot-writer cadence |
| USB monitors: udev + poll fallback, initial enumeration | companion-usb `udev_monitor.rs`, `poll_monitor.rs` | port nearly verbatim (serialport crate is already cross-platform) |
| Per-device driver: write queue + ack, 20 ms read poll, line broadcast, state machine | companion-usb `device.rs`, `state.rs` | port as-is (0018 domain 1) |
| Registry: re-probe backoff (5 s → 60 s cap), dispose discipline | companion-usb `registry.rs` | port as-is |
| Identity handshake: `* HELLO` on boot, `I` fallback, 4 s compile-time assert, legacy detection, tolerant prefixes | firefly `probe.rs` | adopt verbatim, widen family check to `proto == "suzu/1"` |
| Three-domain split + Law of Instances | COMPANION-0018 | the architecture law for all new crates |
| Liveness by session/probe, never PID | COMPANION-0016 | serial sessions make it trivial: the session *is* liveness |
| Host-is-the-read-model; deltas only on the stream | COMPANION-0014 | the self-report agent and mixer assume it from day one |
| Wire-kind anti-corruption boundary | companion-sdk `core_payloads.rs` | per-producer translator modules in `suzu-producers` |
| Testing published up-front (Mock transport, Recording adapter, harness) | companion-sdk `testing/`, Book IX | repeat: fixtures land in M0, not at the end |
| Faces, scenes, slot layouts | all four firmware families | the reference firmware is a *port*, not a rewrite |

**Built fresh (none of it existed anywhere):**

1. **The mixer** (`suzu-host`): multi-producer ingestion (SSE pull + HTTP
   push), source registry with hue assignment, arc handles, budgets
   (per-source gain, bus limit, digest overflow), ground tenancy
   (alert > run > ground), restore semantics. *The genuinely new
   engineering — and it lives host-side on purpose.*
2. **Producer zero**: the self-report agent (cross-platform metrics) and
   the webhook shim (POST → envelope).
3. **Coverage handshake**: declare grounds/rings/slot-layouts/budgets at
   session start (the PoC's identity, widened).
4. **suzu-fit**: behavioral conformance (autonomy, comms-loss honesty,
   restore-after-ring, degradation to invariant skeletons).
5. **The terminal face** (vesper): the zero-BOM companion.

## 2 · Workspace layout (OS-agnostic)

```
crates/
  suzu-core/              # the language: message inventory types, invariants,
                          # arcs, valence/urgency/phase. Zero I/O, fully
                          # property-testable.
  suzu-wire/              # suzu-t codec: frame parse/serialize, arity tables,
                          # XOR checksum. suzu-b (COBS+CRC8+CBOR) behind a
                          # feature flag, later.
  suzu-host/              # the mixer: producers in, tenancy, budgets, arcs,
                          # hue registry, routing decisions out.
  suzu-session/           # companion sessions over any byte transport:
                          # handshake, coverage, slot writer, re-send window.
  suzu-usb/               # monitors + device driver + registry (domain 1 of
                          # COMPANION-0018; the ported companion-usb).
  suzu-producers/         # self-report agent, webhook shim, SSE pull client;
                          # per-producer envelope→signal translators.
  suzu-companion-sdk/     # OPTIONAL rust companion library. Small by design:
                          # the contract is the SDK; this is a convenience.
bins/
  suzud                   # the host daemon (service-friendly; loopback web API)
  vesper                  # terminal face + the companion's own rake
  suzu                    # operator CLI: adopt, fleet, fit
  suzu-fit                # behavioral conformance harness
firmware/
  suzu-a/                 # minimal reference tier (§4)
  suzu-d/                 # template tier (§4)
```

Dependency discipline: the staged workspace deps (tokio, serde, uuid v7,
chrono, tracing, async-trait, futures) are the PoC's exact set — keep it.
Platform-specific code stays behind `#[cfg]`: `udev` on glibc Linux,
poll monitor everywhere else (the PoC's musl/Windows/macOS rule). axum
for the loopback web API; sysinfo for producer zero.

## 3 · The Resident is a DDD monolith

One process, many bounded contexts — the garden's ARCH-0017 playbook
scaled down to an appliance, with COMPANION-0018's Law of Instances
inside. Domain hygiene is what makes the single-exe tradeoffs
(in-process isolation, per-device concurrency, self-update) mitigated
by construction.

### The domains

| Domain | Aggregate(s) | Owns | Failure posture |
|---|---|---|---|
| **sensor** | `Machine` | per-OS environment capture; ground-source zero | a failed metric degrades that slot to unknown; never trips the domain |
| **moments** | `ArcBoard`, `Budget` | the visitor door; tenancy (alert > run > ground); coalescing; alert latching | overflow → digest ring; never blocks the sensor |
| **sessions** | one `CompanionSession` **per connected device** | transport, the state machine (New → Evaluating → Accepted/Rejected → Disposed), coverage, outbound frame queue | one wedged port dies alone with its reason recorded; re-probe backoff; the house keeps running |
| **identity** | `Roster` | names, hues, device_id continuity | read-mostly; survives everything |
| **adoption** | procedures + catalog | flash checklists; verb-scoped (`suzu adopt`) | elevated once, in its own process; the Resident never holds privileges |

### The hygiene rules (the report-before-tripping contract)

1. **One owner per state.** Cross-domain traffic is bounded queues with
   policy (coalesce or drop) — never shared locks, never cross-domain
   mutation. Each domain is an actor with an inbox; within a domain,
   state is single-owner and deterministic.
2. **Panics are contained at the domain boundary.** Every domain task
   runs under a supervisor: panic → capture → restart with backoff.
   One crashed domain is one crashed domain.
3. **A domain reports before it trips.** Degrading domains publish
   `degraded(reason)` with last-known-good state *before* unwinding —
   the rest of the house learns what went bad, and the log reads like
   a story instead of a stack dump.
4. **Devices are individual objects.** One aggregate instance per
   connected device, each with an explicit state machine and its own
   outbound queue. Sessions never share state; concurrency exists only
   as disciplined queues, so nothing races by construction and anything
   can be queued where necessary.
5. **The Resident never holds privileges.** Adoption is a separate
   verb, elevated once, in its own process; self-update replaces the
   file and restarts the (unprivileged) service.

### The managed object model

Everything managed is a domain with a manager; every managed thing is
an object with identity, states, events, and methods.

```text
DeviceManager
  └── devices[]                    every connected device, as an entity
        Device
        ├── identity   device_id · name · hue · class (signature match)
        ├── state      New → Evaluating → Accepted/Rejected → Disposed
        ├── events     appeared · departed · degraded · changed · …
        ├── methods    identify() · firmware.install() · firmware.update()
        │              factory_wipe() · rename() · tell(moment) · …
        └── queue      outbound frames, one owner, no races
```

- **Pipelines receive one device and handle it.** A pipeline is a
  checklist (`hardware/classes/<id>/procedure.yaml`) bound to a single
  `Device` for its whole run: `device.firmware.install()` starts the
  install pipeline, which steps through `erase → write → verify →
  probe`, updating `device.state` as it goes. The state machine guards
  entry — a pipeline only starts from a compatible state, only one
  pipeline per device at a time, everything else queues.
- **Device events are the moments source.** `appeared` rings discovery,
  `departed` rings the toll, `degraded` raises the alert — the
  management object model feeds the bells directly. The house hears
  what the manager knows.
- **Device methods are every surface at once.** The adopt menu calls
  them today; `suzu tell` calls `tell()`; MCP tools derive from them
  later — one object model, many mouths, the same law as the command
  manifest.

### The watcher handoff

The watcher domain senses new and gone USB devices and runs the
identification ladder — and then **ends its cycle**. It never manages.
If the signature matches, it *asks the device domain to mind a new
device* with the facts it gathered; if not, it ends its cycle and keeps
listening. Ownership transfers once; the watcher retains only the
presence ear.

```text
watcher:  sensed(port) → identify(port) → facts
            ├── signature match → devices.mind(facts) → cycle ends
            ├── no match        → report unknown → cycle ends
            └── unplugged mid-probe → cycle ends (nothing to mind)

watcher:  sensed(gone, port) → devices.gone(port) → cycle ends
```

The handoff payload is the identification facts: port handle, USB
descriptors, transcript verdict, matched class. From that moment the
**device domain owns the individual**: it constructs the `Device`
entity, transitions `New → Evaluating → Accepted`, opens its session,
and is the sole authority over it until `gone`.

- **Unplug** → the watcher notifies `devices.gone(port)`; the device
  domain maps the port to its minded device, runs the pipeline-abort
  if one holds it, transitions `→ Disposed`, and rings the departure
  onward. Identity is retained — the roster remembers the individual,
  so a replug later is a *homecoming* (`Accepted`, name and hue
  restored), not a stranger.
- **Port names are not identity.** The watcher identifies *before*
  handoff, so the device domain receives firmware-level facts, not a
  port name to trust. CH340 twins and port reuse are handled by
  construction.
- **Unplug mid-pipeline** (e.g. mid-flash) → pipeline aborts cleanly,
  device → `Disposed`; a half-flashed board that returns in bootloader
  mode re-enters through the same ladder and can resume via the
  recover procedure.

### The communication law

Domains talk in exactly three ways — everything else is forbidden:

1. **Commands** — external methods on a domain's manager or aggregates:
   `devices.mind(facts)`, `device.tell(moment)`, `pipeline.abort()`.
   Imperative; always routed to the owner.
2. **Events** — published facts, past tense: `appeared`, `departed`,
   `degraded`, `changed`. The moments domain subscribes; the bells hear
   what happens.
3. **Cheap objects** — immutable snapshots for cheap checks:
   `device.status()`, `adapters.table()`, `machine.health()`. Copies of
   state taken in one small step by the owning domain — never a live
   reference into another domain.

The rule that gives cheap checks their teeth: **the inspect path can
never wedge the working path.** The `suzu` device table, the servicing
menu, the health lines, and the detective read only cheap objects — a
wedged port, a pipeline mid-flash, or a sensor hiccup is unreachable
from the read side by construction.




## 4 · The official firmware tiers

One language, coverage classes by device budget — `suzu-a` and `suzu-d`
are **subsets of the same message inventory**, not dialects. The firmware
must know what its device can afford; the coverage declaration is where
it says so.

### suzu-a — "the ember" (extremely limited)

Target: any MCU with a UART — ESP-01, ATtiny-class, 8-bit — and any
simple output (1 LED, a few pixels, a buzzer).

- **Wire**: suzu-t only. Receiver = 64-byte line buffer + five opcodes:
  `I`, `K`, `G` (full set only — no deltas; state fits in registers),
  `R`, `X`, plus `OK`/`ERR`. No `D`, no `J`, no frames.
- **Faces**: two or three, baked: breath (ground), the tempo alphabet
  (timer-ISR blink words), the comms-loss face. **Labels are ignored** —
  a-class renders hue + tempo only, which is precisely the
  invariant-skeleton degradation, implemented in firmware.
- **State budget**: ground slots ≤ 8 bytes, ring latch 1 byte, line
  buffer 64 bytes. Flash budget: parser + tempo table + breath LUT.
- **Autonomy**: breath runs in the timer; 5 minutes without `K`
  transitions to the declared comms-loss face (breath continues, tempo
  marks the solitude).
- This tier is the **conformance floor**: suzu-fit core fixtures must
  pass on the cheapest device we certify.

### suzu-d — "the ledger" (roomy)

Target: ESP8266/ESP32-class with a display (the PoC fleet, minus
nothing).

- Everything in **a**, plus: `D` delta slots; `J` JSON escape (ujson on
  ESP8266 is proven by the PoC); label slots rendered as text; the run
  ground with progress; the hour context with night-dimming policy;
  full coverage sheets with named slots and update budgets.
- **Templates**: the roomy tier's privilege. A template is a face
  *layout* with bound slots — the diorama is one (`report-dual` with six
  slots; `run-bar` with three; `toast` with two). The firmware stores
  layouts; the host fills values; the coverage declares which templates
  exist and their slot contracts. Layout intelligence lives host-side
  (asymmetry principle); the firmware only fills.
- **Frames** (`P`/`F`/`C`/`B`) are an optional *capability flag* on
  suzu-d (matrix-class), not a third tier.

Porting note: the four PoC firmware families are the reference
implementations — rp2040-matrix → d+frames, OLED v1/v2 → d, T-Display →
d. The faces are already written; this is a port with a new handshake
(`suzu.json` descriptor: proto, companion, family, variant, coverage
class, device_id), not new invention.

## 5 · Milestones

Borrowing the epic's discipline: each milestone lands green, hardware
gates where it matters, the Discovery Mandate applies (the plan is a
hypothesis).

| M | Deliverable | Gate |
|---|---|---|
| **M0** | `suzu-core` + `suzu-wire`: inventory types, arity tables, codec; **conformance fixtures defined** | property tests green; fixtures run against a golden vector set |
| **M1** | `suzu-session` + virtual companion: full session loop against an in-process device | handshake → coverage → ground → ring → restore → comms-loss, all in CI, zero hardware |
| **M2** | `suzud` mixer v0: producer zero + webhook shim + tenancy + budgets; **vesper terminal face** | one producer, one terminal companion, live on a laptop |
| **M3** | official `suzu-a` firmware on real ember hardware | suzu-fit core passes on-device |
| **M4** | `suzu-d` port of the OLED v2 + diorama faces | the fleet renders the quiet report |
| **M5** | cricket + tunes; matrix frames engine | completion rings audible; bloom visible |
| **M6** | web API + MCP surfaces; suzu-fit full suite; promote inventory + wire into CONTRACT.md as suzu/1 | the contract is the law, in code |

## 6 · Honest risks

1. **The mixer semantics are unproven design.** Tenancy, budgets, and
   digest behavior will meet reality in the first week — expect suzu's
   own "device-bus moment" (COMPANION-0012) and let the Discovery
   Mandate work.
2. **macOS was never soaked** in the PoC (poll monitor covers it, but
   nothing lived there). M2 on a Mac is a deliberate early test.
3. **MicroPython RAM ceilings on suzu-d**: `J` parsing needs the PoC's
   discipline (fixed buffers, ujson, slot count limits). The coverage's
   budget declaration is the guardrail.
4. **The mixer is where scope gravity lives** — every feature wants to
   be "just a little routing logic." The bell is the scope; the model
   doc wins arguments.

## 7 · What this plan refuses to do

- No host-side pixel composition (faces are the companion's).
- No binary-by-default (the asymmetry principle).
- No PID files, no systemd coupling, no platform-native service managers
  (0016's lesson, generalized).
- No feature before its conformance fixture exists (Book IX, learned the
  hard way).

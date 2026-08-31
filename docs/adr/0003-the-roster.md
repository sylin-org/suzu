# ADR-0003: The roster — device lifecycle, maintenance sagas, and the
# admission gate

**Status:** Accepted · **Date:** 2026-08-30 · **Decider:** the Keeper
**Applies to:** the Resident (roster, devices, maintenance), the
workbench, every suzu face class

## Context

The Resident streamed to whatever port answered the handshake. Three
failures came of that on one bench night: a face under a dead session
kept "minded" status while receiving nothing; a freshly-flashed face
would rejoin the stream before anyone knew whether it worked; and
nothing distinguished *the individual* (a device_id) from *a port it
happened to sit on this week*. Meanwhile "factory reset" existed
nowhere — not as procedure, not as data — despite the tool's own law
that the tool which bricks is the tool that un-bricks.

## Decision

**The roster owns the lifecycle; subscription is a granted gate, not
ambient behavior.**

The individual (device_id) moves through:

```
Discovered → Convalescing → Streaming ⇄ UnderMaintenance → Convalescing
                                                                    ↘ Retired
```

- **Live is granted exactly one way**: an admission test passed
  (handshake, ack law, label round-trip where the face declares a
  label, and a display-truth assertion captured through the J shot and
  decoded per the class manifest). Prior trust never skips the test —
  a homecoming individual re-enters New like anyone else. The
  card shows the verdict ("Working"), never the checklist; the
  checklist lives in the Log, where evidence belongs.
- **Maintenance is a saga**: the session closes (one master per
  port), steps are journaled as house events, and the saga keys on
  device_id — a replug mid-flash is the saga waiting, not a failure.
  Two levels, declared per class in `procedure.yaml`: `soft` (face
  files to ship state) and `factory` (full erase, runtime rebuilt,
  identity restored).
- **Backup precedes every write**, and the runtime artifacts are
  vendored (`firmware/artifacts/`, checksummed) so the factory path
  works offline; a missing artifact fails the saga before any erase.
- **Latched alerts outlive their face**: the latch lives in the
  domain, so a face wiped mid-alert re-raises the alarm after
  admission.
- **The roster is a pure domain** (no serial, no sockets) with a
  shared read model; transports subscribe to its verdicts
  (`StreamAttached` / `StreamDetached`). The admission verdict is the
  only key that opens the gate.

## Consequences

**Positive.** "Device under maintenance stops receiving everything"
is one detached subscription, not three special cases. "Fresh device
joins only after tests pass" is the only path into Streaming. The
step-by-step the workbench shows *is* the saga journal — no parallel
progress model to drift. The display-truth step makes the trail
camera the QA instrument: verify, then say so, mechanically.

**Negative / accepted costs.** Every session open now costs an
admission exam (~4–8 s) before ground flows — accepted: the faces
were offline anyway. A face whose class declares no frame law cannot
take the display-truth step and is admitted with it marked skipped.
The estate of vendored artifacts grows with each class.

**Rejected.** Port-keyed lifecycle (a replug orphans the saga — we
tried ports as identity; the bench rejects it nightly). An admission
bypass flag for "trusted" devices (the flag always ends up set
forever). Sagas declared only in code (the procedure files are the
class's own record of what maintenance means).

## Amendment — the deed is written at adoption (2026-08-30)

The original decision wrote a minted identity to the device "when
firmware is installed." Between installs, nothing persisted it: every
resident restart re-read a descriptor with no `device_id`, minted a
new one, and the roster grew a ghost per boot — three fresh
individuals for one COM12 on a single bench night.

- **The deed rides the session, not the saga.** A face minted at
  first sight receives its `device_id` through the same wire that
  feeds it (`J,{"device_id":…}`), the moment its session opens —
  before any exam verdict. Identity precedes trust: a face that
  fails its exam keeps the name it will pass the retry under.
- **The face holds the deed.** `suzu.json` on the device is the
  record; the descriptor answers from it, so the second boot says
  the name the first was given. A face that cannot store (no
  writable state, an ancestor dialect) simply stays on the old
  path: mint per session, no persistence, honestly visible as such.
- **The install path is unchanged.** Sagas still write identity on
  install — now as one writer among the same deed, not the only
  one. Factory reset remains the deliberate exception that carries
  the identity across the erase.

## References

- `docs/the-face-contract.md` — the face side of the same honesty
- `hardware/classes/*/procedure.yaml` — the declared steps
- `crates/suzu/src/resident/{roster,admission,maintenance}.rs` — the
  domain, the exam, the sagas
- The rule-zero incident (`esp8266-oled-v2/manifest.yaml` class
  notes) — the reason maintenance must be a ceremony, not a command

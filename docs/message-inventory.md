# The suzu/1 message inventory

*Draft: the shared language — every message that earns its place, its
invariants, its slots, and its wire form.*

Draft date: 2026-08-28. Promotion path: ratified sections graduate into
`CONTRACT.md`; until then this document is the working proposal. Under
[`the-model.md`](the-model.md) (division of labor) and
[`wire-protocol.md`](wire-protocol.md) (framing, classes, asymmetry).

---

## 1 · Design rules applied

1. **Minimal on purpose.** Core messages are permanent promises. The
   inventory admits only what the harvest proved and the use cases need.
   Extension happens in the extension tier (§10), not by quietly growing
   the core; core changes require a version bump (`suzu/2`).
2. **Slots, not streams.** Field *names* never cross the wire. Each
   companion declares its slot layout (order + arity) in its coverage at
   the session; the host then sends bare values in that order.
3. **Context lives in the session.** Version, source identity, hue
   assignment, arc handles: negotiated once at the handshake. A packet
   carries boundary, kind, payload, integrity — nothing else.
4. **Attribution is host-resolved.** Producers speak `source`; the
   companion receives the *already-mixed* ring with hue and label
   resolved. The serial hop never carries producer names.
5. **The companion owns its slot layout.** Two companions may hold the
   same ground with different slot orders; the grammar doesn't care.

## 2 · Grounds (state class — self-healing, idempotent)

Three grounds. Presence is *not* a separate ground: at low tiers,
`report` renders as breath — the same message, told at the device's
depth.

| Ground | Meaning | Slots (tier-scoped; companion declares subset + order) |
|---|---|---|
| **report** | the face at rest: "alive, and here is my truth" | subject(name), health(thriving/wilting/withering/offline), uptime, cpu, mem, disk, io, offerings, stones, seed_bank, hour |
| **run** | claimed work ground (The Run); tenancy class `run`, preemptible by alert | label, progress(0–100), hue |
| **rest** | deliberate rest — owner-commanded dark; distinct from absence and from comms-loss | — (resume on next ground.set) |

Rendering is the companion's business: `report` with subject+health only
*is* breath on a pixel, hearth on a matrix, the status line on an OLED,
the diorama on a TFT. Slumber is not a ground — it is companion policy
applied to the `hour` context (§5). The `health` slot is a **fold** of
the companion's status atoms (§3): levels 5–4 → thriving, 3 → wilting,
2–1 → withering, 0 → offline.

## 3 · Data atoms, vitality, and the fold law

Vitality is a **number, not an enum** — 0–5, bad to good — with one
off-scale value for horizontal information:

```
6   INFO — horizontal: off the vitality axis; excluded from folds
5   operational        2   failing
4   strained           1   stopped (deliberate — frost, rest)
3   degraded           0   offline / absent (the honest zero)
```

The words are *readings* of the scale, not the wire form. Companions
map bands to their own rendering policy: a three-value device reads
5–4 as thriving, 3 as wilting, 2–1 as withering, 0 as offline — the
PoC's health vocabulary was this scale, coarsely quantized, all along.
Ring urgency uses the same scale (tempo gloss: 5–4 breath, 3 pulse,
2 blink, 1 strobe, 0 dark).

**The datum.** One message carries a self-describing datum:

```
S,<set>,<axis>,<level>[,<fraction>[,<min>,<max>,<unit>]][,<text>]*hh\n
S,os,disk,5,2.5,0,2,TB,50GB out of 2TB used\n
S,host,name,6,-,-,-,-,stone-gentle-giant\n
```

- `<set>.<axis>` is the datum's **identity** — `os.disk` names the
  part; the level is orthogonal and updates in place. (`OK.disk` is
  axis `disk` at level 5: the level-word notation is a *reading*,
  never the key.)
- `<level>`: 0–6 as above. Horizontal atoms (6) carry descriptions —
  names, kernels, versions — and are invisible to folds.
- `<fraction>`: fixed-point percent ×100 (0–10000) — the gauge
  position in integer math. `<min>,<max>,<unit>` and `<text>` are
  optional presentation slots: the ready-made string for text
  surfaces; range and unit for displays that draw their own axes.
- **Coverage declares the slot depth**, per companion: `lean`
  (axis+level), `gauge` (+fraction), `full` (everything). The host
  sends exactly that — capable displays get the whole datum; smaller
  ones degrade toward the level; the humblest see only the fold.
  This is the user's example made protocol: on a very capable display
  all the slots land; on smaller UIs it degrades toward the core
  level.
- Byte note: atoms are state-class and idle-timed — metrics breathe at
  seconds, not frames — so 40–70 bytes at 115200 baud is nothing. The
  frame path never carries atoms.

**Sets** remain declarative vocabulary packages (the tune pattern):
YAML, overlay-able, publishable by anyone; companions declare fluency
by name and never need the file. **A set may add axes; it may never
touch the scale.**

**The fold law.** `fold(atoms) = min(level ≤ 5)` — the worst part
wins, horizontal atoms invisible. The worked example: gpu=5,
network=5, storage=3 folds to **3 — degraded**. The fold feeds the
report ground's `health` slot on folded-coverage companions, and a
host may additionally speak it as a moment for surfaces that live in
rings.

**Fluent vs folded coverage.** A companion declares, per set, either
`fluent` (holds N atoms; renders per-part — a status list is the
diorama's offerings rows with health dots, which the PoC firmware
already draws) or `folded` (receives only the fold). The asymmetry
principle in semantic form: the host reduces; the ember just glows
the truth.

**Ring qualifiers ride the same ladder.** Rings may carry set
qualifiers — `alert.thermal`, `discovery.bt-peer` — and degrade by
**prefix truncation**: strip qualifiers from the right until the name
is known; the root verb is always known. The name itself is the
degradation ladder; no negotiation is required to be understood.

## 4 · Rings (moment class — sequenced, at-least-detectable)

Nine. The valence/urgency/phase matrix of everything a house needs to
hear.

| Ring | Valence | Urgency | Phase semantics | Story |
|---|---|---|---|---|
| **heartbeat** | neutral | silent | periodic | "still here" — the tick on devices whose ground doesn't visibly pulse |
| **begin** | + | low | opens an arc | a long thing started |
| **completion** | ± (outcome slot) | low | closes an arc | it ended — `outcome: ok` is the water-can; `outcome: fail` is the sad ending |
| **discovery** | + | low | event | something new appeared (planted, replanted, peer seen) |
| **departure** | − mild | low | closes an arc | something went away (goodbye, expired, uprooted) — the toll |
| **alert** | − | high, sustained | opens an alert arc | needs attention; the only sustained-fast state on any device |
| **allclear** | + | low | resolves an alert arc (must carry its arc) | healed — the crossfade home, the heal tune |
| **tended** | + | silent | event | the human touched it — the reciprocal gesture |
| **transition** | neutral | low | event | a state changed (rested, woke, started, stopped) |

Rules:
- Ring `<urgency>` is the **same 0–5 scale** as vitality (§3), rendered
  as tempo by device class — one scale to learn, everywhere.
- **Failures decompose into two messages.** A *failed ending* is
  `completion` with `outcome: fail` — the attempt closed, render the sad
  ending (a dark blink, a low tone). An *ongoing problem* is `alert` —
  it latches until resolved. "Backup failed" is both, in sequence: a
  completion{fail} for the attempt, an alert for the condition.
- **alert** latches until its `allclear` or host `X`. If the host dies
  mid-alert, the alert *stays* — danger persisting unconfirmed is the
  honest failure mode.
- **momentary rings** (completion, discovery, tended, transition,
  heartbeat, begin, departure) render for their scene's natural duration
  and return; sustained rings (alert) hold.
- **Moments borrow the face; states own the truth.** A moment overrides
  the ground for its natural duration and the ground resumes — nothing
  is cleared. Because low-urgency moments are brief and budget-capped,
  a distressed ground (a slow disk-degraded blink) is *interrupted*,
  never starved: after the drive-inserted bloom, the blink comes back.
  And the moment may herald a state change that heals the fold — the
  new drive's atom lifts the blink back to breath.
- Every ring carries an **arc handle** (session-scoped, host-assigned
  0–255). `allclear` must reference its alert's handle; the companion
  resolves the arc, not a "state".

## 5 · Context and session

| Message | Class | Meaning |
|---|---|---|
| **hour** | state | the house clock, pushed on the hour boundary — drives skies, Slumber dimming |
| **keepalive** | state | host liveness, ~1/min; its absence is how the companion's comms-loss face is triggered deterministically |
| identity / coverage | session | the handshake: proto, companion, family, firmware, **coverage** (grounds + rings declared, slot layouts, budgets, `enc`) |
| ack | session | `OK[,...]` / `ERR,<reason>`; carries seq echo for moments |

## 6 · Wire forms (`suzu-t` arity table)

```
I                                   → OK,{proto,companion,family,firmware,coverage*hh\n
K\n                                 → OK\n
Z,<hour>\n                          → OK\n
G,<ground>,<slot values in declared order>\n
D,<idx>:<value>[,<idx>:<value>...]\n
S,<set>,<axis>,<level>[,<fraction>[,<min>,<max>,<unit>]][,<text>]*hh\n
X\n                                 → restore ground after ring/run overlay
J,{json}\n                          → complex-value escape (snapshots, rich config)
R,<signal>,<urgency>,<hue>,<arc>,<seq>[,<label...>]*hh\n
P,<x>,<y>,<r>,<g>,<b>\n   F,<r>,<g>,<b>\n   C\n   B,<percent>\n     (frames capability only)
OK[,<seq>]\n / ERR,<reason>\n
```

- Strings: final slots only; printable characters excluding `,` `*` CR
  LF. Anything richer → `J`. The host transliterates (it computed the
  label anyway).
- `*hh` XOR checksum required on `I`, `G`, `J`, and any transport of
  config; optional on `R` hot path (seq + grammar carry integrity).
- Frames (`P`/`F`/`C`/`B`) are the PoC vocabulary verbatim — the
  frame-capable devices keep their host-side engine.
- `suzu-b` (COBS+CRC8) encodes the same table: kind byte + CBOR payload;
  nothing else changes.

## 7 · Reliability per message

| Class | Messages | Mechanism |
|---|---|---|
| state | G, D, S, X, J, Z, K, P, F, C, B | at-most-once, idempotent; loss self-heals on the next delta |
| moment | R | ack + seq echo; host re-sends once after 300 ms silence; companion dedupes by (arc, seq), remembering the last 8 |
| session | I, coverage, ack | checksummed, retried, 4-second deadline |

## 8 · The heal-moment mapping (zen-garden → suzu/1)

| Garden event | suzu/1 | Notes |
|---|---|---|
| StoneSeen | heartbeat | ground.report carries presence continuously; heartbeat is the explicit tick |
| StoneGoodbye / expired / uprooted | departure | the toll |
| OfferingPlanted / Replanted | discovery | |
| OfferingRested / woke | transition | |
| CaptureCommitted | completion (arc: capture run; begin opened at run start) | |
| HealthDegraded | alert (opens alert arc) | |
| HealthHealed | allclear (resolves the arc) | |
| Tended | tended | |

## 9 · Genericity: why the alphabet closes

The test for genericity: can any story a *machine* has about itself or
its world decompose into the inventory without new core messages? The
event ontology closes:

- something **appeared / disappeared / changed** → discovery / departure
  / transition;
- something **started / ended well / ended badly** → begin / completion
  {ok} / completion {fail};
- something **needs attention / no longer does** → alert / allclear;
- a **part has a status** → a set atom, folded for the humble (§3);
- **still here** → heartbeat and the report ground;
- **a human acknowledged** → tended.

Human-scale examples land without new kinds: a doorbell is a discovery
with urgency; a chat message is a discovery labeled "ada"; a timer is
begin → completion; a threshold crossing is an alert with a label. Two
boundaries are deliberate, and they are where the language *should* be
narrow:

1. **No content channel.** Rings signal that something exists; the
   content itself (messages, media, data) travels by other means. Suzu
   says "ada spoke," never carries the conversation. (This is the line
   against becoming a notification system.)
2. **No general content grounds.** Grounds are the machine's truth about
   itself. An app may visit with rings; only `run` is claimable as a
   ground. Ambient content streams (weather feeds, dashboards-as-content)
   are other products' jobs — the boundary against scope gravity.

## 10 · Extension: primitives, sets, qualifiers

Core messages are reserved words, versioned. Everything else extends
through **sets and qualifiers** (§3) — and attribution never enters the
namespace at all:

- **Producers adopt or publish sets** and speak `<verb>.<qualifier>` —
  `alert.thermal`, `discovery.bt-peer` — or data atoms
  `S,os,storage,3,87.2,0,2,TB,1.75TB used`. No registry ceremony, no
  version bump, no waiting.
- **Unknown qualifiers degrade by truncation.** A companion that has
  never heard of `alert.thermal.sustained` renders `alert.thermal`;
  one that knows neither renders `alert`. The root verb is always
  known — *the name itself is the degradation ladder.* Status atoms
  degrade by the fold: a companion fluent in no set still receives the
  folded primitive in its health slot. Nothing degrades to nothing.
- **Coverage is the negotiation.** The handshake declares which sets a
  companion is fluent in, how many atoms it holds, and which qualified
  ring names it knows; the host routes rich forms to the fluent and
  roots/folds to the rest. suzu-fit tests the degradation, not just
  the rendering.
- **Private ranges for IDs.** In `suzu-b`, kind bytes `0xF0–0xFF` are
  the vendor page (the USB-HID pattern); core IDs below are assigned
  with the core.
- **Graduation path.** A set that proves universal is adopted into the
  core distribution (the tune pattern: filesystem sets already overlay
  embedded ones); a qualifier that proves core-worthy is promoted at
  the next version. The registry grows by adoption, not by decree.

Note what this settles by construction: **producer identity lives in
slots (hue, label), never in signal names.** Semantics name the world;
slots name the speaker.

## 11 · Deliberately absent from suzu/1

- **Per-device special kinds** — devices express variety through
  *coverage and faces*, not through private opcodes. (Frames are the one
  sanctioned capability class.)
- **Query/read kinds** on the serial hop — the host already holds state;
  the companion's truth is `OK`/`ERR` and its faces. Diagnostics belong
  to vesper/web surfaces.
- **Priority in the envelope** — the host mixes; the companion receives
  the winner.
- **Anything visual** — no scene names, no layouts, no colors beyond the
  hue/urgency slots.

The inventory is open for extension by version bump, closed for silent
drift — the same law as the contract itself.

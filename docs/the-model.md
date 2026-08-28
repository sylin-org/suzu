# The model

*The settled mental model: producers speak stories, companions own faces,
the contract is only the language.*

Settled: 2026-08-28, after the ideation trail in
[`poc-companion-surface.md`](poc-companion-surface.md),
[`prior-art-and-positioning.md`](prior-art-and-positioning.md),
[`presence-catalogue.md`](presence-catalogue.md), and
[`applications-and-sharing.md`](applications-and-sharing.md).
This document is the constitution the later work converges on. When older
docs contradict it, this one wins.

---

## 1 · The division of labor

| Layer | Owns | Never touches |
|---|---|---|
| **Producers** | stories — semantic events in the shared language; the editorial choice of which moments deserve a ring | presentation, devices, other producers |
| **Companions** | faces — pre-cooked, highly-optimized presentations; engines; degradation ladders; their own silicon's budgets | other companions, routing, the meaning of messages |
| **The contract** | the language — message inventory, invariants, usage law | faces, layouts, scene names, anything visual |
| **The host** | mixing — routing, budgets, tenancy, arbitration — all in the language | pixels; it is a mixer of meanings, not of frames |

The politics match the runtime model (guests at the edge own themselves):
taste lives with the maker closest to the medium. The contract keeps the
language small; the foundries compete on timbre.

## 2 · The language (what the contract owns)

- **The envelope** — kind, source, timestamp, subject, body; plus `arc` +
  `phase` for stories (begin → sustain → resolve).
- **The message inventory** — rings and grounds, each with its
  **invariants**: presence/absence, health, valence, urgency, story phase,
  attribution (source, hue), the hour. The invariants are what must
  survive any rendering, on any device, at any capability.
- **Slot parameters** — hue, tempo, label, progress, hour. Data lands in
  declared slots, never as content streams.
- **The usage law** — per-source gain, a global ring budget with digest
  overflow, ground tenancy (alert > run > ground), restore-after-ring,
  and the courtesy rule (programmatic clients speak rings and ground;
  raw commands are human- and agent-grade).

What the contract deliberately does **not** own: scene names, layouts,
color prescriptions (beyond invariants), animation engines, fallback
chains, where the rendering engine runs.

## 3 · Faces (what companions own)

The music-box principle. Constrained devices (ESP8266, ESP32) cannot be
display drivers, so companions ship **repertoires**: pre-composed,
pre-optimized faces burned into firmware. The host never streams content —
it calls a face and feeds its slots. The PoC's tricks are the house style:
precompute at identity time (gauge sprites, icons, hue palettes), tick-based
choreography, lookup tables over math, dirty regions, and — most
importantly — **self-running faces** that need zero host contact.

Consequences held as requirements:

- **The host provides data; the device provides life.** A companion runs
  its ground with no host present.
- **Comms-loss is a face.** Declared, honest, marked — never a silent
  freeze and never a silent blank.
- **Determinism where memory demands it** (seeded starfields) — which
  makes per-companion visual regression diffable.
- Faces are where the device maker's taste lives. The contract protects
  this by never reaching below the language.

The earlier proposal to standardize a scene catalog in the contract is
**retracted**: companions declare *coverage* ("I can tell `alert`; I hold
the report ground; slot updates ~10/s"), never their scenes.

## 4 · Grounds and silence

Silence is three different things, never conflated:

1. **System quiet, device alive** → the **ground**: the face at rest,
   modality-native — silence (audio), breath (pixel), hearth (matrix),
   the quiet report (text/status: name, cpu, disk, uptime), the diorama
   (page). The default state of the product; most of its life is spent
   here.
2. **System quiet, device dead** → the **absence**: legible as wrong
   *only because* the ground is normally present.
3. **System speaking** → rings and peals, layered over the ground
   temporarily, then restore ("ring, then return").

Ground frames are first-class protocol citizens (`ground.set` /
`ground.delta`), not residual state. The presence catalogue's grounds are
scenes with slots — firmware truth, not streamed compositions.

## 5 · Modality as a family of registers

One shared *semantics*, per-modality *registers*. A tune is not the audio
tier of a bloom; light and sound are two compositions sharing one meaning.
Each modality composes in its native strength:

- **light owns states** — persistent, peripheral, glanceable;
- **sound owns sequences** — tempo, rhythm, urgency in time;
- **text owns precision** — names, numbers, timestamps;
- **pages own narrative depth** — the whole arc, annotated.

Working position on routing: **chorus for rings** (moments may be echoed
across the room), **specialization for grounds** (states live where they
are native). Chorus inherits the budget.

## 6 · What this settles

- **Conformance is behavioral.** suzu-fit asserts meaning-level response
  to scripted stories: something changed, alerts persist until resolved,
  grounds restore after rings, the comms-loss face appears, budgets are
  respected. It never asserts pixels. Visual regression is
  per-companion, enabled by deterministic faces.
- **Degradation ladders are local policy.** How a rich face falls back to
  a poor one is the companion's affair; the contract requires only that
  the invariants survive the downgrade.
- **The bridge is a binding table.** WLED, AWTRIX, ESPHome and the whole
  commodity ecosystem already speak in faces with slots; mapping shared
  messages onto their native scenes makes them suzu companions without
  new firmware.
- **The matrix is not a special case.** A host-side sprite engine is
  simply that companion's private choice of how to build its faces.
- **The presence catalogue's grounds are scenes** — read its per-device
  renderings as repertoire entries with slots, not as host-composed
  streams.

## 7 · What remains open

Two concrete pieces of work, both contract-writing rather than
philosophy:

1. **The minimal message inventory for `suzu/1`** — which rings and
   grounds earn their place, with which invariants and slots. Small on
   purpose: every message is a permanent promise.
2. **The behavioral conformance suite** — the fixture set that tests the
   language without ever seeing a face, including the autonomy and
   comms-loss clauses.

---

## 8 · Ubiquitous language

Suzu is not its ancestor's companion edition. The harvest is provenance —
we read ancestor code, benefit from its lessons, and cite it in history
docs — but its vocabulary never enters suzu's language:

- **firefly** is suzu's moniker for *visual* devices; **cricket** for
  *audio*. They are species names in suzu's own taxonomy, not references
  to any ancestor project.
- Ancestor firmware gets the **temporal tag**: *pre-suzu firmware* — a
  device state, not an identity. Devices are never "zen garden things";
  they are fireflies and crickets waiting to speak suzu.
- `zen-garden`, PoC numbers, and ancestor ADR ids may appear in history
  and harvest docs (`docs/poc-companion-surface.md`); they must not
  appear in class files, tool output, descriptors, or any artifact that
  a contributor reads as *the* language.

---

*Bells from different foundries have nothing in common in timbre — that
is the face, and timbre is why you love one bell and not another. But
everyone in the valley understands rung, tolled, pealed. Faces are
timbre. The language is meaning. Suzu keeps the language small and lets
the foundries compete.*

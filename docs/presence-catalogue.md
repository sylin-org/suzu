# The presence catalogue

*Candidate multimodal presences for the hardware the PoC actually shipped —
steady-state grounds and rings, rendered at every tier.*

Design date: 2026-08-28. Builds on the signal lexicon
([`prior-art-and-positioning.md`](prior-art-and-positioning.md) §6).
Every capability cited here is grounded in the PoC firmware in
`../zen-garden/firmware/firefly/` — file and behavior noted inline.

---

## 0 · Ground rules

Three rules govern every presence in this catalogue. They are the calm
budget applied to steady states:

1. **The ground may drift; only regime changes ring.** Load wanders, the
   sparkline moves, a count ticks. None of that is a ring. A ring means
   the *regime* changed: a threshold crossed, a state entered or left, an
   arc begun or resolved. Coalescing, applied to expression. Without this
   rule, a text/graph steady state would nag — which is precisely what
   text and graphs tempt you to do.
2. **The ground never goes dark while the story is healthy.** A dark
   device must *mean* something: offline, unprovisioned, or asleep. The
   lowest ground (one pixel breathing) exists so that "alive" is always
   distinguishable from "silent."
3. **One glance answers "all is well?"; the second glance tells the
   story.** Every ground carries its state in its most glanceable channel
   (tempo for mono, hue for color, headline for text) and its detail one
   layer deeper.

## 1 · The fleet, as the firmware actually is

| Device | Panel | What the firmware can truly do | Expression vocabulary |
|---|---|---|---|
| **RP2040-Matrix** | 5×5 WS2812 RGB | 25 pixels, 30 fps animations (rainbow/pulse/chase/sparkle/blink), global brightness, status fills, boot sweep | *glyphs, blooms, sprites, tempo* |
| **OLED v1** (ESP8266) | 128×64 SSD1306, **hardware dual-color**: yellow header rows 0–15, blue body rows 16–63 | profont text, 8×8 health glyphs (filled/hollow/X), CPU/MEM progress bars, scrolling name, `wipe()` two-line reveal transitions, contrast `pulse()` | *headline + two bars* |
| **OLED v2** | same panel | dense dashboard: icon+bar rows (CPU/MEM/DSK), info column (offerings/net/clock/stones), 8×8 Open Iconic glyphs, **2×3 activity spinner that advances one step per arriving event** | *the report that mostly doesn't change* |
| **T-Display** (ESP32) | 135×240 ST7789 color | three-panel diorama: identity bar **hue-hashed from the stone's name**, health text in sage/honey/clay, 4 gauges filled from a pre-rendered cold-to-hot rainbow sprite, **time-of-day sky scene driven by the hour field**, offerings list with health dots, capability icons with scanning-underline busy state, and a NO_COMM ambient mode (midnight sky, 12 seeded twinkling stars, 3 fireflies on Lissajous orbits) | *the page* |
| **Single pixel** (candidate tier-0; any spare GPIO) | 1 mono LED | on/off, PWM fade | *tempo only* |
| **Cricket** | any speaker | 4 channels, YAML tunes, per-event debounce | *silence and peals* |

Two firmware discoveries that shape everything below:

- **The ground/ring split already exists in the wire protocol.** `J` (full
  snapshot), `L` (incremental load), `D` (dashboard frame), `M` (metrics)
  are *ground frames* — they set steady state. `T`, `WIPE-IN/OUT`, `+/-`,
  `SD/SR`, `A` are *ring commands* — momentary or transitional. The
  lexicon doesn't invent this distinction; it names it and makes it
  contract.
- **The house already knows what time it is.** The presence payload has
  carried an `hour` field end-to-end, and the diorama's sky redraws from
  it. Night behavior (§5) is a policy change, not new plumbing.
- **Stones own a color.** `stone_hue(name)` hashes the stone's name to a
  deterministic hue, matched with the web UI. Identity hue is a lexicon
  token waiting to be named: your stone *is* that color, everywhere.

## 2 · The canonical ground: a server reporting itself

Scenario: `stone-tranquil-pass`, thriving, 62 days up, 5 offerings, seed
bank attached, Tuesday 14:00. One producer, six renderings of the same
tranquil message — *everything is quiet and alive.*

### Breath — tier 0 (single mono pixel)

A sine breath, 8-second cycle, brightness swinging 10–60% — never off,
never full. Presence without demand. That is the entire rendering: the
pixel is the stone, breathing. Nothing to read; something to feel. At
this tier the *tempo alphabet* (§4) carries every story; steady state is
its slowest word.

### Ember — tier 1 (a few RGB pixels)

One pixel breathing in the stone's **identity hue** (sage-green by
convention for "the stone itself"), plus one green pixel when the seed
bank is present, one blue when services run — the PoC's own color
assignments, now a constellation of what the stone *has*. Drift in load
may scale breath amplitude slightly. That is all.

### Hearth — tier 2 (5×5 matrix)

The PoC's baseline, kept because it is already right: one to three
warm-white firefly sprites wandering the matrix with ease-in-out fades;
load raises their tempo and density; a green firefly joins for the seed
bank, a blue one for services. Steady state reads as *fireflies at dusk* —
which is the whole point: the correct rendering of "a healthy server" is
not a number, it is a mood.

### Ledger — tier 3a (OLED v1, dual-color text)

```
┌──────────────────────────┐
│ TRANQUIL-PASS        (y) │  yellow zone: name (scrolls if long)
├──────────────────────────┤
│ ●  THRIVING       62d 3h │  blue zone: health glyph + copy + uptime
│ CPU ▮▮▮▮▮▁▁▁▁▁    42%    │
│ MEM ▮▮▮▮▮▮▁▁▁▁    61%    │
└──────────────────────────┘
```

The steady state *is* the heartbeat here: the refresh timestamp and the
bar drift are the pulse. Note the firmware's gift — the header zone is a
different color than the body, so the glance channel (name) is
physically separated from the report channel (numbers). The ground never
wipes; `wipe()` is reserved for rings.

### Ledger v2 — tier 3b (OLED v2 dashboard)

Same panel, denser: icon+bar rows for CPU/MEM/DSK left, info column
right (offerings count, net, clock, stones), bolt icon for the seed
bank. And the subtlest liveness signal in the fleet: the **2×3 spinner
that advances only when an event arrives**. At true steady state it
rests. A completely still dashboard and a breathing pixel are the same
message at two depths; the dashboard merely adds *how much* is quiet.

### Diorama — tier 4 (T-Display)

```
┌─┬────────────────────────┐
│▌│ STONE                  │
│▌│ tranquil-pass          │
│▌│ ● thriving      62d 3h │
│▌│ ────────────────────── │
│▌│ CPU ━━━━━▁▁▁▁▁▁    42  │  cold→hot ramp fill
│▌│ MEM ━━━━━━━▁▁▁▁    61  │
│▌│ DSK ━━▁▁▁▁▁▁▁▁     30  │
│▌│ I/O ━▁▁▁▁▁▁▁▁▁     12  │
│▌│░░░░░░ (14:00 sky) ░░░░│  scene band, day regime
│▌│ OFFERINGS              │
│▌│ ● redis     ● searxng  │
│▌│ ● qdrant    +2 more    │
│▌│            🌱  ⚙       │  seed bank + capability icons
└─┴────────────────────────┘
```

This is the report from a server the user asked for: **graphs** (four
gauge bars filled from the rainbow sprite, value text turning clay only
past 80), **text** (name, health, uptime, offering names), and the
household's clock (the sky band) — all already implemented in
`diorama.py`. The candidate refinement for suzu: let the scene band
double as a **sparkline strip** by day — the last hour of load drawn as a
thin landscape under the sky — so the page holds *recent time*, not just
*gauge now*. The sky remains the sky; the hills beneath it become the
last hour of load drawn as a thin landscape.

### Quiet — audio ground

Silence. The tranquil default; cricket unmutes only for rings. The tune
system's ambient/background channels exist for houses that want a night
loop, but the catalogue's position is the delight budget's: silence is a
feature, and a steady state that plays sound has already failed.

## 3 · Rings across the fleet

The lexicon's ring vocabulary, rendered per tier. Mono devices carry
urgency in **tempo**; color devices in **hue**; text devices in the
**headline**; the matrix in **motions**.

| Ring | Tier 0 pixel | Tier 1 pixels | Hearth 5×5 | Ledger v1 | Ledger v2 | Diorama TFT | Cricket |
|---|---|---|---|---|---|---|---|
| **heartbeat** | breath-tick (one soft pulse) | hue pixel brightens briefly | a sprite brightens as it passes center | uptime ticks; no wipe | spinner rests — silence *is* the heartbeat | sky advances an hour | — |
| **completion** | double-blink, then breath | quick bloom across pixels, fade | warm full-field bloom, 1.5 s, then baseline | `wipe("CAPTURE DONE", "09:12 · 4f2a")` | spinner advances one step; bar settles | toast line + **checkpoint tick** on the scene strip | water-can tune, foreground |
| **discovery** | triple-blink | a new pixel joins the constellation | new sprite spawns at edge, wanders in | `wipe("NEW OFFERING", "redis planted")` | offerings count increments (brief invert) | new dot pops into the offerings row | planting chime |
| **departure** | slow 2 s fade to off, hold dark 10 s, then re-breath | pixel fades over 5 s | sprite floats to the edge and fades | headline swaps to `GONE QUIET / emerald-vale` | stones count decrements | offering dot greys; stone line dims | soft goodbye tune (the toll) |
| **alert** | **fast blink until resolved** — the one sustained fast state | amber pulse, slow | sprites turn amber, tempo rises | body glyph flips to X; health line inverts; bar of the offender highlighted | warning icon replaces heart; offending bar blinks | health dot → honey; gauge value clay; scene strip shades the period | alarm, foreground (debounced) |
| **healed** (transition) | crossfade back to breath | amber → hue crossfade | amber crossfades to warm white, tempo settles | `wipe("HEALED", "03:12")`, then ground | warning → heart | health → sage; "healed 03:12" annotation on the strip | heal tune |
| **rested** | brief slow blink, then breath | pixel dims, stays in constellation at half | one sprite dims out; the crowd thins | `wipe("RESTING", "redis")` | bar row drops out quietly | offering dot → dim | single low note |
| **tended** | one polite blink | white micro-flash | sparkle pass across the matrix | `wipe("TENDED", "by rake")` | spinner flourish (3 quick steps) | small ripple on the identity bar | wind-chime |

Three catalogue decisions worth defending:

- **Alert is the only sustained fast state on tier 0.** Everything else
  resolves to breath. Fast-blink is the tier's entire emergency range,
  so it must stay rare to stay meaningful.
- **Departure includes darkness, briefly.** The tier-0 pixel going dark
  for ten seconds makes absence a rendered event, then returns to breath
  (the *stone* is gone; the *device* is still yours).
- **Completion annotates graphs.** The diorama's checkpoint tick and the
  shaded degraded period (below) are the page tier's storytelling
  superpower: graphs that remember why they look the way they do.

## 4 · The tempo alphabet (tier 0 and all mono devices)

Five named tempos; nothing else allowed. A mono device's entire dynamic
range, kept small on purpose so each word stays distinct:

| Word | Rate | Meaning |
|---|---|---|
| **breath** | 0.125 Hz (8 s sine) | alive, well — the ground |
| **tick** | one pulse per 30 s | heartbeat mark (multiples are fine: 60 s, 5 min) |
| **pulse** | 1 Hz ×3 | notice me, politely (completion, tended) |
| **blink** | 2 Hz sustained | needs attention (alert) |
| **strobe** | 4 Hz sustained | emergency; the top of the range, rarely used |

On dual-color OLEDs, the alphabet extends with **invert** (momentary
inverse video) as the flash primitive, and on the matrix with the
already-built animations. The principle is modality-independent:
*urgency lives in time when you have no hue to spend.*

## 5 · Three more candidate grounds

### Slumber — the night ground

The `hour` field already flows to every device. When the house sleeps
(policy constant, e.g. 23:00–07:00): the pixel's breath deepens to 12 s
at lower amplitude; hearth sprites slow and dim; OLEDs drop contrast
(the v1 firmware's `pulse()` already proves contrast animation works)
and the diorama needs nothing at all — **its NO_COMM mode is already a
night ground**: midnight sky, twinkling stars, drifting fireflies.
Rings at night shrink one register: completion becomes a single soft
pulse and cricket stays silent, with the morning carrying the digest.
The house sleeps; the garden keeps watch quietly.

### Flock — the multi-stone ground

Steady state for the *garden*, not a stone. Tier 0 inverts the usual
logic: **dark means all well** (an andon inverted — no news as the
good news), any hue pulse means *something* needs attention. Hearth:
one sprite per stone (25 pixels hold a dozen stones comfortably, each
wandering in its stone's identity hue). Ledger: rotates
worst-first — the headline cycles through stones, longest-since-seen or
least-healthy first, ten seconds each, so a text panel covers a fleet
by rotation. Diorama: the foot panel's offering rows become stone rows;
the rake wall monitor's garden panel (per-peer chirp freshness) is the
proven desktop version of exactly this.

### The Run — a work-in-progress ground

For long operations (capture, restore): a *temporary* ground that is
neither rest nor ring. Hearth becomes a slow blue chase (the firmware's
`chase` animation, repurposed); Ledger's progress bar becomes run
progress; Diorama's gauges swap one row for run progress + rate; tier 0
uses **tick at 5 s** (a work beat, distinct from the 30 s heartbeat).
Completion resolves the run with its ring; the ground returns to
whatever was before. The Run is the bridge between ground and peal: a
story whose middle is long enough to need its own steady state.

## 6 · A peal, walked through the house

One story — mongo degrades, then heals — told by every device in time
order. This is the catalogue's acceptance test: if the narrative
survives every tier, the lexicon works.

```
t0  alert fires
  pixel:   breath → blink (2 Hz, sustained)
  hearth:  sprites amber, tempo rises
  ledger:  glyph → X, "WILTING · disk 91%"
  ledger2: warning icon; DSK bar blinks
  diorama: health → clay; gauge 91; scene strip begins shading the period
  cricket: alarm (once; debounce holds it to one voice)

t1..tN  sustain (the story's present tense)
  pixel keeps blinking — the only device that never stops telling it
  text keeps the headline; the page keeps shading; the strip grows

t2  healed
  cricket: heal tune
  ledger:  wipe("HEALED", "03:12") → ground
  hearth:  amber crossfades to warm white, tempo settles
  pixel:   blink → breath
  diorama: health → sage; strip's shaded band ends; annotation "healed"
  ledger2: warning → heart; the spinner does one unhurried step
```

Every tier told the same three acts — something went wrong, it lasted, it
ended well — at whatever depth it holds. The tier-0 pixel told it in
tempo alone. That is the lexicon passing its own test.

## 7 · What this catalogue asks of the contract

1. **Name the ground/ring split** the wire protocol already makes:
   ground frames (`ground.set`, `ground.delta`) distinct from rings
   (`ring.<signal>`), with the ground carrying the steady-state payload
   (identity, health, load, counts, hour) and rings carrying arcs.
2. **Standardize the lexicon tokens the firmware proved out**: identity
   hue (hash of name), the five-word tempo alphabet, sage/honey/clay
   vitality colors, invert as mono-flash, wipe as the text transition,
   the hour field as the house clock.
3. **Extend identity/manifest with narrative capability** observed here:
   `zones` (OLED dual-color), `sprites` (8×8), `gauges` (count + ramp),
   `scene` (hour-driven), `spinner` (event-tick liveness), `holds`
   (ring | sequence | headline | page).
4. **Fix the content policy in normative SHOULDs** (§0): drift-silent
   grounds, the one-sustained-fast-state rule, darkness must mean
   something, worst-first rotation for fleet text panels.

---

*Grounded in: `firmware/firefly/circuitpython/code.py`,
`micropython/firefly_oled.py`, `micropython/v2/firefly_oled_v2.py` +
`icons.py`, `micropython/tdisplay/diorama.py`, and
`installer/NewFirefly.ps1` (provisioning and descriptor).*

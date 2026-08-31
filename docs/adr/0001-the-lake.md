# ADR-0001: The lake — raindrops, atom fireflies, and the face contract

**Status:** Accepted · **Date:** 2026-08-29 · **Decider:** the Keeper, with the suzu author
**Applies to:** all suzu faces; the rp2040-matrix face first

## Context

The 5×5 (Waveshare RP2040-Matrix, 25 WS2812 pixels, per-pixel color and
intensity, CircuitPython on 264 KB SRAM / ~1 MB drive) joined the fleet
running the old PoC firefly firmware. The fleet needed: a real suzu/1
firmware for the class, an adoption flow (`suzu prepare`), and a decided
rendering grammar for how *moments* look on a light face.

Three renderings were debated for moments on the 5×5:

1. **Centered wipes** — ripples from the panel's center, category color,
   start/end edges. Rejected: claims the whole face, suspends everything,
   and needs collision rules the moment two moments arrive together.
2. **Identity-by-position atoms** — CPU/MEM/GPU pinned to home pixels,
   urgency shown as hue-shift. Rejected by the Keeper: loses the delight
   of fireflies coming and going in different places.
3. **Raindrops in a lake** — moments land at random centers as colored
   ripples that expand and fade; concurrent drops overlap and add.
   **Accepted.**

For the working state itself, value-as-position was also rejected in
favor of **value-as-tempo**: each numeric atom is a firefly whose
*breathing speed* is its value.

## Decision

**The lake.** The face is a pond. The machine's numbers are fireflies
that breathe with its load; moments land as raindrops — a bright impact,
then a colored ring expanding and fading in the category's hue. Light
adds: overlapping drops and fireflies compose without rules. The
fireflies remain while ripples pass — moments borrow the face; states
own the truth.

Per the face contract (`docs/the-face-contract.md`):

- **No data = idle.** Silence returns the face to its garden. The garden
  opens the show at boot.
- **`label` is contract-reserved on every device**: persisted, restored
  at boot, never reverted, stamped by one writer.
- **Rings speak the nine verbs** (alert, allclear, completion, discovery,
  begin, departure, tended, transition, heartbeat) with qualifier
  degradation — `alert.disk` matches by verb; unknown qualifiers never
  disconnect. **Alert latches** — the lake keeps ringing at its drop
  point until `allclear` lands (at the same point) or the host `X`s.
- **The host reduces.** The serial hop carries the device's dialect
  only: light sentences for the matrix (hue = valence, intensity =
  urgency, tempo = the gloss, pattern = the story), text+icons+tempo for
  the OLED, its own voice for ancestor faces.

**Recording stays host-side** (the trail camera): the face never buffers
or stages — each screenshot is one stateless in-band reply, and `suzu
record` loops it at a wire-respecting rate.

## Consequences

**Positive.** Idle and working are one visual language — fireflies at
rest, fireflies at work — so the idle→working transition is the garden
waking, not a mode switch. Concurrency composes for free (light adds).
The ring's "start" and "end" animations are physics: an impact flash and
a decaying wave. The 5×5 becomes the most literal renderer of the
vocabulary: urgency *is* intensity, valence *is* hue.

**Negative / accepted costs.** Per-item attribution is given up on the
canvas — the panel reads as one energy, and attribution arrives through
rings (`alert.disk`) and labels when it matters. Base64 replies cost
the face ~120 ms of blocked write per screenshot (accepted; the wire,
not the encoder, is the tax — see `docs/install-lessons.md`). The
render is now layered (atoms + ripples + rain compositing per tick),
which is a little more code than a fill.

**Alternatives rejected.** Centered wipes (claims the face; concurrency
rules). Identity-by-position (loses the wandering delight — the
Keeper's call). Flash-staged recording (the poor side pays wear, RAM,
and modes). Source-compiled faces on ESP8266 (compile peak OOMs the
heap — bytecode + raw data files only).

## References

- `docs/the-face-contract.md` — the contract this ADR commits to
- `docs/message-inventory.md` §3–4 — vitality scale, tempo gloss, the nine rings
- `docs/wire-protocol.md` — the asymmetry principle
- Bench provenance: the OLED's bytecode lesson, the matrix's
  remount-aware installer, and the record GIF (`record-COM12.gif`)

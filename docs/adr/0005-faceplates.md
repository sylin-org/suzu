# ADR-0005: Faceplates — the dress and the hang

**Status:** Accepted · **Date:** 2026-08-30 · **Decider:** the Keeper
**Applies to:** every face class with a display; the Resident's
maintenance sagas and read doors; the workbench's install ceremony

## Context

The esp8266-oled face was composed for one mounting: USB at the
bottom, the yellow label band on the right, reading downward. Hung
the other way — connector up, the way a shelf actually wants it — the
same pixels hang upside down. The face code already ran its whole
composition through one portrait transform (`pixel(u,v) → oled(v,
63-u)`), so an inverted variant was a flip, a band direction, and an
area order away — and the keeper wanted the choice surfaced, not
buried in a rebuild: a small menu with captured previews, worded for
someone who knows which way their USB stick points and nothing of
bytecode.

## Decision

**A faceplate is a declared bundle with a name for humans, a mount
for the wall, and a preview that was captured — never drawn.**

- **The declaration grows the human side.** `faceplate.yaml` carries
  `display_name` ("BIG!" — shareable across variants), `blurb` ("easy
  to read from a distance"), and `mount` — one of a fixed geometric
  vocabulary: `usb-down`, `usb-up`, `usb-left`, `usb-right`. The id
  (`name`) stays wire-canonical: directories, `suzu.json`, and the
  wire speak ids; display names and captions are manifest-side and
  never shipped to the device.
- **Mount is the axis of choice between siblings.** Variants sharing
  a `display_name` are one face with several hangs; the chooser's
  question is the mount, answered with a pictogram — drawn by the
  workbench from the declared value (board outline, OLED, connector
  at the declared edge), never shipped per faceplate. Captions carry
  the physical meaning in the keeper's own words.
- **Previews are captures.** Each bundle may carry `preview.gif` (or
  `.png`), recorded from a working face in its intended orientation.
  A missing preview degrades gracefully: the pictogram and the words
  carry the choice alone. Nothing in the model requires a preview to
  exist.
- **The variant is derived, not forked.** One face source, one
  orientation constant (`INVERT`), a build script that generates the
  sibling's `face.py` and compiles its bytecode (`based_on` names the
  parent; the derived bundle says so and is regenerated, never
  hand-edited). The constant costs one branch per draw call — no JSON
  parsing, no runtime flags on the 80 KB-heap board.
- **Choice travels as data through the existing ceremony.** The
  maintenance sagas accept a `faceplate` parameter, validated against
  the class's declared vocabulary and refused by name when unknown;
  the push writes the chosen bundle and `suzu.json` records the id;
  the saga announces the faceplate in its own step voice. A class
  that declares no faceplates behaves exactly as before — the
  parameter is simply absent.
- **The chooser lives in the ceremony.** Install/Reinstall show the
  declared faceplates (preview GIFs looping, pictogram and blurb
  beside); a live face gains a light swap action — files and a nudge,
  no bootloader — and, per ADR-0003, every dress returns through the
  admission exam before the pill goes LIVE.

## Consequences

**Positive.** The question the user can actually answer — "which way
does it hang?" — is the one the UI asks. Faceplate #3 costs a
directory and a declaration; the chooser, the doors, and the saga
parameter are class-agnostic. Previews are captures, so the menu
shows working faces, and the swap's receipt is the live face itself.

**Negative / accepted costs.** Two compiled bundles per face mean two
`mpy-cross` runs and two artifacts to keep built; the build script is
the one place that knows. The orientation constant adds a branch to
the proven face's draw path — accepted, the default path is
behaviorally identical. A faceplate whose preview has not been
captured yet presents itself by words and pictogram alone — accepted,
grace is the requirement.

**Rejected.** A runtime orientation flag read from `suzu.json` (the
board would parse JSON at boot to draw — heap it doesn't have);
hand-drawn mounting images per faceplate (four pictograms drawn once
by the workbench beat a gallery of drifting art); forking the face
source into two files (the first fix applied to one and not the other
was inevitable); a generic "rotate any face" mode in the tool (art
lives in the files, not in the tool — ADR-0003's provisioning law).

## Amendment — the currency gate (2026-08-31)

A face worn in an outdated dress joined the stream as readily as a
current one; nothing told the keeper, and nothing told the face.
Amended:

- **The declaration carries a `version`.** The descriptor a face
  reports is the version of the dress it actually wears; the
  declaration states the version the house now ships.
- **Currency is part of worthiness.** The admission exam gains a
  first step: worn older than declared fails it, and the stream
  waits — the refusal names the remedy ("update the faceplate"),
  and the workbench card offers it as a button. A declaration
  without a version, or an unreadable one, asserts nothing.
- **The update is the ceremony that already exists** — a soft saga:
  files, a nudge, and the exam again, now passing.
- **A stale Suzu face updates itself.** A face that already speaks
  suzu/1 and wears a declared dress that is merely older needs no
  keeper's hand: the house starts the soft saga for the same dress
  the moment the exam refuses it, bounded to two attempts per
  attach — a dress that will not bump must not loop the house.
  Ancestors, undeclared dresses and factory resets keep their
  ceremony; this is housekeeping, not adoption.

## References

- `docs/adr/0003-the-roster.md` — the ceremony every dress re-enters through
- `docs/the-door-contract.md` — the envelope the chooser's doors speak
- `faceplates/esp8266-oled-v2/portrait-numerals/faceplate.yaml` — the first declared face
- `crates/workbench/ui/app.js` — the chooser and the pictograms

# ADR-0006: The say and the substrate

**Status:** Accepted · **Date:** 2026-08-30 · **Decider:** the Keeper
**Applies to:** every face class; the Resident's sessions, moments and
control chirp; the CLI; the workbench

## Context

The faces streamed machine statistics, and rings existed as frames on
the same wire — with no stage to stand on. A ring's icon was erased
by the next ground frame a second after it appeared; "identify" was
an injection into a wire, not a delivery to a face. Meanwhile the
keeper named what suzu is actually for: applications calling a CLI
method or an API endpoint and putting a message on hardware — a 5×5
matrix splashing orange, a small TFT composing an icon and the full
sentence, an OLED speaking in its yellow strip. Same utterance,
different styles. The stats were never the product; they are what a
face does between sentences.

## Decision

**The communication is layered. The substrate fills the gaps; the
ring owns the stage; the say is semantic; presentation is the
faceplate's own decision.**

- **The substrate fills the gaps.** Ground and pulses flow when
  nobody is speaking, and are the resting texture of every face. If
  no say ever arrives, the house is a very honest system monitor —
  complete, and beside the point.
- **The ring owns the stage.** Delivery is an exchange with the face,
  not an injection into the wire: the session pauses the substrate
  (holding the freshest ground, dropping nothing that matters), the
  face presents the message in its own style for its own duration,
  and the face announces **DONE** when the message is *integrated* —
  a momentary ring when its bloom ends, a latched alert as soon as
  its steady state can take ground without unpainting the message.
  The host holds a generous bound and resumes regardless: a face
  that cannot answer cannot hold the stage.
- **The say is semantic — and degrades at the instance.** The wire
  from the house to the instance carries signal, urgency and words —
  never pixels. The **faceplate's declaration states what its face
  can speak**: qualified signals or bare verbs, a text channel or
  none, whether it can announce DONE at all. The *instanced device*
  degrades accordingly: a 5×5 matrix that speaks bare verbs receives
  `R,WARN` — the qualifier and the message dropped, less bytes, less
  noise, and the face never sees a frame it cannot use. What
  `WARN` *looks like* — splash, composed screen, band and blink —
  is the faceplate's own decision. Unknown signals degrade to the
  face's default presentation.
- **Targeted and broadcast are one verb.** `suzu say` is a sentence
  in a small grammar — port, signal, text, in that order, each
  optional after the first. The house resolves a target against its
  own live enumeration: exact port name, device id, unique suffix —
  ambiguous or unknown targets are refused by naming what is known.
  A targeted say rides to one session (`Devices.Get(port).Say`);
  a broadcast say keeps the moments budget. Both take the stage.
- **The sentence is the interface.** CLI, control chirp and HTTP
  mirror one parser and one envelope; `GET /api/device/identify/COM24`
  reads as the utterance it is. The window's Identify button is one
  sentence of the grammar, not a special case.

## Consequences

**Positive.** Applications put messages on hardware with one call and
a known vocabulary; every dress presents in its own style and proves
delivery with DONE; rings stop being papered over by the substrate;
identify and say share one mechanism and one grammar; port naming
follows the host OS because the house resolves targets against its
own enumeration, never against patterns.

**Negative / accepted costs.** Every face bundle must speak DONE
(regenerated through the faceplate build); the session gains a stage
and a held-ground slot — bounded, per-session, invisible when idle;
the grammar reserves the ring and level words, so free text beginning
with a signal word reads as a signal (the escape is a qualifier or a
rephrase); the substrate to a presenting face is paused, so its
numbers may lag by the length of the message.

**Rejected.** Host-composed pixels for says (presentation is the
faceplate's art, ADR-0001's asymmetry turned to the room); host-timed
stage duration (the face owns its moment; the host's bound is a net,
not a metronome); a second, say-specific wire message (the R frame
was already semantic — the stage was the missing half); OS-specific
port patterns in the grammar (the enumeration is the truth, and it
already knows).

## References

- `docs/adr/0001-the-lake.md` — the asymmetry principle, turned to the room
- `docs/adr/0004-the-door-and-the-store.md` — the watched lane that
  keeps the stage honest, and the door contract the say speaks
- `docs/adr/0005-faceplates.md` — presentation is the faceplate's decision
- `faceplates/esp8266-oled-v2/portrait-numerals/face.py` — the first face
  that says DONE
- `crates/suzu/src/control.rs` — the chirp ear that parses the sentence

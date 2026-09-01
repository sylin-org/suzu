# ADR-0008: The onboarding lifecycle — prototype in Python, promote to
# Rust

**Status:** accepted  
**Date:** 2026-09-01

## Context

Support for a new board arrives in two very different media. The first
contact is exploratory: which bootloader does it have, what quirks does
its bridge inject, which baud latches, what breaks the boot. Python is
the right material for that conversation — quick to write, quick to
rewrite, and the reference tooling (esptool, mpremote) speaks it. The
second medium is the Resident: a service that must run for months, be
installable as one binary with no interpreter, and treat a half-written
flash as a routine Tuesday.

Until now the path between them was archaeology. The esp8266 runtime
flash lived as a Python procedure for its whole life; when the host
interpreter was removed (2026-08-31), the recipe was ported to Rust in
one large step, and the bench immediately taught what the port had
glossed over — a sync flood that masquerades as command replies, a
response layout read from the wrong offset, and a vendored image that
boot-loops unless its header is patched to the detected flash size.
Those lessons are now codified in `bootloader.rs`, but nothing in the
repo says that codifying them is *the expected step*, or defines when a
procedure has earned promotion.

## Decision

**Every device procedure walks one lifecycle, and its stage is declared
where the procedure lives.**

```
prototype ──→ verified ──→ promoted
 (python)     (bench)      (native rust)
```

- **`prototype`** — the executable truth is a script under `scripts/`,
  kept out of the Resident's path by construction. The procedure is
  declared in the class's `procedure.yaml` *as it is discovered*, with
  `via:` naming the script — so promotion is a translation of a
  stabilized spec, not a redesign. New prototypes share the serial and
  REPL conventions instead of copying them; duplication of the pusher
  class across scripts is the smell that says the shared helper is
  overdue.
- **`verified`** — the script has survived real hardware end to end:
  the full procedure completes, admission passes, and the interesting
  failure paths have been *seen*, not imagined. What the bench taught
  is written into `install-lessons.md` or the class notes with
  provenance, before any port begins ("codify verbatim; no
  improvements").
- **`promoted`** — the Resident speaks it natively. The Python script
  is demoted to a development reference and is never invoked by the
  service again; `procedure.yaml` is rewritten to the native recipe
  with the script kept as an annotated ancestor reference.

The stage is declared per procedure as `status: prototype | verified |
promoted` in `procedure.yaml` — one file remains the procedure of
record, rather than the truth scattering into READMEs and comments.

### The promotion bar

A procedure moves to `promoted` when all of these hold:

1. **Pure core, thin transport.** Everything that can be wrong without
   hardware — codecs, framing, state machines, block math — is split
   from the serial layer and unit-tested against wire fixtures
   (byte-exact, with provenance comments). Serial-touching code stays
   thin and is honestly untested by CI.
2. **Constants with provenance.** Bauds, timeouts, chunk sizes, reset
   dances: named, commented with where they came from, and never
   "improved" during the port. The bench paid for each one.
3. **Failure paths enumerated.** Framing loss aborts loudly; an
   interrupted write leaves the device recoverable by re-running the
   same procedure — the tool that bricks is the tool that un-bricks.
4. **Wire compatibility stated.** On-device files, API surfaces, and
   event tags that deployed devices already depend on keep their exact
   names across the promotion, with a test pinning them.
5. **Instruments left behind.** `#[ignore]` bench probes and an
   opt-in wire trace ship with the promoted module, so the next
   hardware session starts with tools instead of `println!` roulette.
6. **Docs reconciled in the same change.** `procedure.yaml` rewritten
   to the native recipe; README and artifact provenance updated; the
   ancestor recipe preserved as commentary.
7. **Admission green on real hardware**, end to end, after promotion —
   the ESP8266 runtime flash of 2026-09-01 being the template (backup
   → ROM loader → JEDEC-detect header patch → 629/629 blocks → first
   handshake → admission passed).

## Consequences

- Promotion today produces *code*, not engine data: each native
  procedure is hand-written Rust beside its siblings. If the YAML
  procedure engine (`suzu-a` tier) is ever built, this lifecycle
  survives unchanged — promotion would then mean adding native verbs —
  but until then, `procedure.yaml` is the spec of record and the Rust
  is its executor, in that order of authority.
- Scripts under `scripts/` are permanent residents of the repository
  and permanent strangers to the service. Their header comments say
  which lifecycle stage they serve.
- The bar is deliberately expensive. Not every class needs every
  procedure promoted; a class with one adopted unit may keep its
  runtime flash at `prototype` indefinitely, honestly labeled.

## References

- `crates/suzu/src/bootloader.rs` — the template promotion (ROM
  loader, 2026-09-01), including the bench probes it ships
- `crates/suzu/src/repl.rs`, `scripts/push_firmware.py` — the same
  procedure as promoted Rust and as ancestor reference
- `hardware/classes/*/procedure.yaml` — where the status lives
- `docs/install-lessons.md` — the incident record that `verified`
  feeds
- ADR-0003 — maintenance sagas and the admission gate that closes the
  promotion bar

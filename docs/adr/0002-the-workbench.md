# ADR-0002: The workbench — a native window beside the Resident

**Status:** Accepted · **Date:** 2026-08-30 · **Decider:** the Keeper
**Applies to:** suzu surfaces; built on the family system (Ghostlight,
Koi)

## Context

suzu is a fleet manager whose whole product is visual, yet its only
surfaces are a CLI and a face on the shelf. The family already solved
this shape twice: Ghostlight's workbench and Koi's are Tauri 2
windows over a headless daemon, wearing one night-garden design
system. Building suzu's window was a question of *where it lives*,
not *what it looks like*.

## Decision

**A Tauri workbench as the third door — tray, four views (Status,
Log, Media, About), zero device access of its own.**

- **The Resident stays the single writer.** The workbench speaks only
  loopback HTTP to `suzu serve` (127.0.0.1:7899 — status, journal,
  in-band shots, sagas, control). It never opens a serial port; a
  port with two masters is how faces get dropped (proven on the
  bench). The Tauri shell is a thin window + tray: show, quit, open a
  destination, reveal a folder — nothing else.
- **Local-first, same machine.** The API binds loopback only. A
  workbench on another computer is a different product with a
  different threat model, rejected for now.
- **The house style, verbatim.** Night-garden palette, five-step ink
  ramp, one accent as signal (suzu's gold — the bell's own color),
  lampband + tabs, spring motion. The About view wears the published
  trading card: same anatomy, same holo, flavor line
  "The garden keeps breathing.", artwork identical to the tray icon
  so the character cannot drift. The closed-link vocabulary ports as
  law: destinations resolve Rust-side, never in markup.
- **Pages, and what they refuse to do:** Status renders the roster's
  lifecycle and offers maintenance as ceremony (confirm, receipts,
  step-by-step from the saga journal, ending in the admission test) —
  planned-but-unimplemented actions show as honestly disabled. Log
  renders the resident's moment journal (what was asked, by whom, and
  what it became). Media watches faces through in-band shots at the
  wire's honest rate — and when a recording runs, **recording
  subsumes the preview**: the panes show the very frames the GIF is
  taking, because a face should never pay two captures for one
  picture.

## Consequences

**Positive.** One design system now spans three tools; suzu's window
was mostly a port, not an invention. The read API that makes the
workbench possible also serves scripts — the third door is open to
anyone on the same machine. Capture ownership by the Resident kills
the stop-serve-to-screenshot dance for good.

**Negative / accepted costs.** A WebView2 dependency rides with the
workbench binary (the CLI stays lean). Two processes must agree on a
port (7899, env-overridable). The journal is in-memory: it dies with
the Resident, like the pause flag — persistence lands when the
Keeper asks for it.

**Rejected.** A web dashboard served by the Resident (another
listening surface where the daemon lives, and it would follow the
Keeper's phone instead of sitting in the tray). The workbench owning
serial (two masters per port — the bench already voted). Electron
(the family standard is Tauri; weight is a feature).

## References

- `docs/adr/0003-the-roster.md` — the lifecycle the Status page
  renders
- `docs/adr/0001-the-lake.md` — the face the Media page watches
- `sylin-org/koi-desktop` — the port recipe this window follows

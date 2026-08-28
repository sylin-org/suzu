# Suzu (鈴)

A framework for making software felt in the home.

Software produces events — a backup finished, a database started, a stone went
silent, a threshold was crossed. Those events disappear into log files and
terminal buffers. Suzu catches them and translates them into light, sound, and
motion, through small companions that live in the home: a matrix of pixels that
blooms green when a backup commits, a speaker that plays the water-can tune when
a capture finishes, a bell that rings when something needs attention.

The name is 鈴 — a small bell, used in Shinto ritual. Not an alarm. A bell:
present, clear, and calm.

## The model

```
producers                    suzu                     adapters
─────────                    ────                     ────────
zen-garden ──┐
home-automation ──┤          ┌───────────────┐        ┌───────────┐
monitoring ────┼──────────► │ event envelope │──────► │ firefly   │
ci/cd ─────────┘            │ command manifest│       │ cricket   │
                            │ identity       │       │ your thing│
                            └───────────────┘        └───────────┘
```

**Producers** emit events. **Suzu** receives them, decides which companions
care, and routes them. **Adapters** translate events into physical expression —
light, sound, motion.

Zen Garden is the first producer. It will not be the last.

## The contract

Three types, defined in `CONTRACT.md`:

1. **Event envelope** — what a producer says. JSON, versioned, transport-agnostic.
2. **Command manifest** — what a companion says it can do. Declared at startup, discoverable.
3. **Identity handshake** — how a hardware device proves it's a Suzu companion.

Every transport (SSE, CLI, web API, MCP, stdio) speaks this same semantic
protocol. The transport is the wire; the contract is the meaning.

## The five transports

| Transport | Audience | How |
|---|---|---|
| SSE | producers pushing events to Suzu | HTTP Server-Sent Events |
| CLI (`vesper`) | operators, developers, testing | the companion's own rake |
| Web API | portals, integrations, HTTP clients | REST on the companion's port |
| MCP | agents (Claude, home-automation agents) | Model Context Protocol server |
| stdio | embedding, scripting | line-delimited JSON on stdin/stdout |

## The adapters

| Adapter | Modality | Hardware | Status |
|---|---|---|---|
| **firefly** | visual | RP2040-Matrix 5×5, OLED v1–v2, T-Display ST7789 | live-proven in PoC (24 devices minted) |
| **cricket** | audio | any speaker | live-proven in PoC (probe e2e played sound) |
| **your thing** | ? | ? | the contract is open; the SDK is optional |

## Methodology

This project follows a **design-first, code-second** discipline:

1. **Harvest** — read the PoC code (`src/poc/` in zen-garden) to understand
   what was proven. Read Zen Garden's event sources to understand what the
   first producer says. Read this file and `CONTRACT.md` to understand the
   protocol.
2. **Ideate** — sketch the surface. What does the CLI look like? What does
   the MCP server expose? What does the web API return? What events matter
   to which adapters? Ideation cycles until the surface feels right.
3. **Implement** — only after the surface is designed.

Do not jump to code. The PoC's implementation is evidence of what works,
not a template to copy. The contract is the law; the implementation serves
it.

## Harvest sources

The PoC code lives in `../zen-garden/src/poc/`. Read these, fully:

| PoC crate | What it proves | Key files |
|---|---|---|
| `companion-sdk/src/` | spawn/lifecycle: SSE-in with backoff, HTTP command server, 50ms coalescing, SIGTERM contract, enabled flag | `builder.rs`, `run.rs`, `adapters.rs` |
| `companion-usb/src/` | USB serial: udev monitor + PollMonitor fallback, per-device reader task, identity handshake | `udev_monitor.rs`, `poll_monitor.rs`, `state.rs` |
| `cricket/src/` | audio companion: YAML tune format, sample management, offline test subcommand | `tune.rs`, `player.rs`, `test.rs` |
| `firefly/src/` | visual companion: three device families, identity handshake, pixel/fill/clear/brightness/animate commands | `orchestrator.rs`, device adapter files |
| `moss/src/domain/companion/` | how the garden manages companions: boot scan, port pool, liveness, registration | `mod.rs` |
| `moss/src/api/v1/companions.rs` | the companion API surface: registration, command passthrough | this file |

Also read (for the producer's perspective):

| Zen Garden file | What it shows |
|---|---|
| `src/v1/crates/moss/src/offerings/events.rs` | OfferingChanged — the event the first producer emits |
| `src/v1/crates/kernel/src/topology.rs` | TopologyEvent — Seen/Goodbye/Expired |
| `src/v1/crates/moss/src/offerings/capture_run.rs` | capture run events (CaptureCommitted) |
| `docs/v1/inventory/clients-companions.yaml` | the PoC companion inventory (live-proven) |
| `docs/v1/inventory/live-poc-harvest-2026-08-28/` | live captures of companion surfaces |

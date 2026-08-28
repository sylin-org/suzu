# Harvest list — what to read before writing code

Read these files fully. They are the evidence base: what was proven, what
worked, what broke, and what the garden (the first producer) actually says.

## The PoC companions (all live-proven, read fully)

| Path | Lines | What it proves |
|---|---|---|
| `zen-garden/src/poc/companion-sdk/src/` | ~2000 | spawn/lifecycle: SSE-in with backoff 1→32s, HTTP command server, 50ms coalescing, SIGTERM contract, enabled flag persistence |
| `zen-garden/src/poc/companion-usb/src/` | ~235 | USB serial: udev monitor (Linux) + PollMonitor fallback, per-device reader task, state machine, RegistryEvent<UsbSerialDevice> |
| `zen-garden/src/poc/cricket/src/` | ? | audio companion: YAML tune format (event-kind → sample + channel + debounce), sample management, offline `test` subcommand, CC0 samples |
| `zen-garden/src/poc/firefly/src/` | ? | visual companion: three device families (RP2040-Matrix 5×5, OLED v1–v2, T-Display ST7789), identity handshake (write `I`, expect JSON in 4s — compile-time assert), commands: status/pixel/fill/clear/brightness/animate/stop/info, 24 devices minted |
| `zen-garden/src/poc/companion-usb/src/udev_monitor.rs` | 158 | Linux udev hotplug detection |
| `zen-garden/src/poc/companion-usb/src/poll_monitor.rs` | ? | cross-platform fallback (no udev) |

## The garden side (how the producer manages companions)

| Path | What it shows |
|---|---|
| `zen-garden/src/poc/moss/src/domain/companion/mod.rs` | companion registry: boot-scan, CommandManifest probe, port pool (7187–7199), liveness, adoption reconcile |
| `zen-garden/src/poc/moss/src/api/v1/companions.rs` | companion API: registration, command passthrough, auto-start, "all" broadcast |
| `zen-garden/src/poc/moss/src/infra/listeners/pulse.rs` | the PoC's event listener infrastructure |
| `zen-garden/src/poc/companion-sdk/src/garden/pulse.rs` | the PoC's garden pulse rendering (used by the companion SDK) |

## The inventory (what the survey found)

| Path | What it covers |
|---|---|
| `zen-garden/docs/v1/inventory/clients-companions.yaml` | all companion capabilities with maturity ratings, live-proven status, and known bugs/drift |

## The live harvest (what the fleet actually said)

| Path | What it covers |
|---|---|
| `zen-garden/docs/v1/inventory/live-poc-harvest-2026-08-28/` | raw captures of companion surfaces exercised against the fleet |

## Zen Garden's event sources (what the first producer says)

| Path | What it shows |
|---|---|
| `zen-garden/src/v1/crates/kernel/src/topology.rs` | TopologyEvent: Seen/Goodbye/Expired (the membership stream) |
| `zen-garden/src/v1/crates/moss/src/offerings/events.rs` | OfferingChanged (the offering lifecycle stream) |
| `zen-garden/src/v1/crates/moss/src/offerings/capture_run.rs` | capture run events (the living will's stream) |
| `zen-garden/src/v1/crates/moss/src/offerings/storage.rs` | bank state changes (the storage stream) |
| `zen-garden/docs/v1/decisions/ADR-0006-suzu-contract.md` | the garden's decision to adopt the Suzu contract |

## What to look for while reading

1. **The event shapes** — what does the garden actually emit? What fields? What granularity? How would you map these to the Suzu envelope?
2. **The transport mechanics** — how does the SSE connection work? What's the backoff strategy? How does coalescing prevent event storms?
3. **The device lifecycle** — how does a USB device go from "plugged in" to "recognized" to "responding to commands"? What happens when it's yanked?
4. **The command relay** — how does `rake hey firefly status` reach the device? What's the proxy chain?
5. **The tune format** — how does cricket's YAML encode event-kind → sample + channel + debounce? What makes a tune feel right?
6. **The identity handshake** — what does the 4-second deadline protect against? What happens when a device answers late?
7. **The port pool** — how were companion ports assigned? What happened at pool exhaustion?
8. **The SIGTERM contract** — how does a companion know to shut down? What cleanup does it do?

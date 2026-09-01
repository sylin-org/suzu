<div align="center">

<img src="res/suzu-mascot.png" alt="Suzu mascot: a small golden pixel-art shrine bell with a serene face, hanging from a red cord on a starry black background" width="100" height="100">

# Suzu (鈴)

**Software, felt in the home.**

Small companions that turn software events into light and sound — a matrix of
pixels that blooms when a backup commits, a speaker that plays the water-can
tune when a capture finishes, a bell that rings when something needs attention.

Part of [Sylin](https://sylin.org) — tools that run on your hardware, show you
their work, and keep running long after you've stopped thinking about where
they came from.

</div>

---

Software produces events all day. A backup finished, a database started, a
stone went silent, a threshold was crossed. Those events disappear into log
files and terminal buffers, and the house never hears about any of it.

Suzu catches them and gives them to the room. Not as notifications competing
for a screen you are already ignoring — as light, sound, and motion from small
companions that live on the shelf, plugged into the same house as the
computers doing the work. A backup finishes across the room, and something
blooms green. Something breaks at 3 a.m., and a bell rings, softly, once.

The name is 鈴 — *suzu*, a small bell used in Shinto ritual. An alarm demands;
a bell announces. The design target is the bell: present, clear, and calm.

## How it works

Three roles, kept deliberately apart:

| Role | What it is | What it knows |
| --- | --- | --- |
| **Producers** | Anything with something to say: a backup job, CI, Zen Garden, a script, an agent | That something happened |
| **The host** | `suzu serve` — the Resident, running on a computer in the house | Which companions exist, how the machines feel, what deserves the room's attention |
| **Companions** | Small hardware faces: a 5×5 matrix, an OLED, a speaker | How to render what the host sends |

The language between them is the **contract**, versioned `suzu/1`. Producers
knock; the Resident answers; the faces render. No producer talks to a
companion directly, and no companion needs to know where an event came from.

## The CLI

One binary. Plain words.

`suzu` with no arguments becomes a watch mode: it polls for hotplug,
identifies what appears, and offers a servicing menu. Everything else is a
verb you can guess. Here is a real transcript, taken on the bench:

```text
$ suzu scan
catalog: 4 class signature(s) from hardware/classes
  COM22          NEW     waveshare-rp2040-matrix (firefly/matrix)
      no identity response — fresh firmware
  COM12          unreachable
      (stale, busy, or non-responding port)
  COM6           non-USB serial port
  ⋮
```

Every device gets an honest state — `NEW`, `fresh firmware`, `unreachable`,
`stale`, `busy` — because adoption starts with telling the truth about what is
plugged in. Adoption itself is one verb, with a backup and a verification
receipt at the end of it:

```text
$ suzu prepare
```

You pick the device, pick a faceplate, confirm, and watch it install. Then the
Resident takes the night shift, and the house learns to speak:

```text
$ suzu serve                                   # the Resident: sense, mind, render
$ suzu say completion A backup committed       # send a moment by hand
$ suzu show INFO.disk Disk at 50%              # or just a string for the face
$ suzu pause                                   # one UDP datagram: hold your breath
```

(`pause` dials `127.0.0.1:7898` — S-U-Z-U on a phone keypad. `suzu resume`
lets it breathe again.)

For an always-on Linux host, install the Resident as an unprivileged systemd
service. The repeatable Debian/Arch/Fedora workflow, verification checklist,
native-package contract, and rollback steps are in
[`docs/linux-installation-playbook.md`](docs/linux-installation-playbook.md).

The whole vocabulary:

| Verb | What it does |
| --- | --- |
| `suzu scan` | Identify every serial port, joined with the hardware catalog |
| `suzu list` | List the Resident's compatible devices; in a terminal, select one to pause, identify, install or change its faceplate, or factory-reset it |
| `suzu detective` | Full fact dump per device, ending in a draft `signature.yaml` for a new board class |
| `suzu serve` | The Resident: watcher, sessions, moments, host sensing, publishing, supervised domains |
| `suzu screenshot [port]` | In-band frame grab from every firefly — no reboot — one manifest-decoded PNG per face |
| `suzu record <secs> <fps> [port]` | The trail camera: the grab loop becomes a GIF of the first answering face, clamped to what the wire allows |
| `suzu prepare` | Adoption: list, choose, back up, install, verify |
| `suzu say <ring> [text]` | Send a moment by hand; `suzu say allclear` heals a latched alert |
| `suzu show <tag> <text>` | Send a display string for the face |
| `suzu pause` / `suzu resume` | Hold and release the Resident, one datagram each |

| `suzu firmware <port>` | Migrate a harvested device to `suzu/1` in place, `device_id` preserved |
| `suzu restore <port>` | Un-migrate from the per-device backup. Refuses without one |

### Managing devices from a terminal

`suzu list` is the Workbench's device ceremony in terminal form. It reads the
Resident's same snapshot and legal-action vocabulary; neither surface opens a
serial port or recreates lifecycle rules. In an interactive terminal, choose a
device and the aggregate offers only actions valid for its current `NEW`,
`LIVE`, or `PAUSED` state. Faceplate selection includes every declared mount
variant, and maintenance steps remain attached until admission returns the
device to `LIVE` or reports a failure.

When output is redirected, `suzu list` prints once and exits. The explicit
forms are useful to scripts, agents, and terminal testing:

```text
suzu list --plain          # one human-readable snapshot
suzu list --json           # the shared Resident read model
suzu list --interactive    # force the selection loop
```

## The contract (`suzu/1`)

The contract is small on purpose: three transport-agnostic message types,
fixed in [`CONTRACT.md`](CONTRACT.md) — an event envelope, a command manifest
("one declaration, many mouths": a CLI generates subcommands, an MCP server
generates tools, a web API generates endpoints), and an identity handshake.
Identifying a companion is one letter: write `I` to the serial port, receive a
JSON hello within four seconds — a deadline the host asserts at compile time,
not a comment.

A companion's whole world is three kinds of thing:

**Grounds** are the state a companion stands on and can hold indefinitely —
`report` (how the house feels: cpu, mem, disk, stones, the hour), `run`
(something is in progress, with a label and a progress), `rest` (nothing
asked; be dark and quiet). Grounds are self-healing: the host re-sends them,
so a companion that missed a frame is correct again on the next one.

**Data atoms** travel on a vitality scale of `0`–`5`, with `6` for plain
information. When several atoms describe one thing they **fold** — worst part
wins. A machine with nine healthy disks and one dying one reports the dying
one, because that is the atom the room needed.

**Rings** are the nine moments that splash over a ground, linger, and fade:

| Ring | The moment it names |
| --- | --- |
| `heartbeat` | all is well; presence, continued |
| `begin` | something started |
| `completion` | something finished (with an outcome; failures decompose into `completion{outcome:fail}` + `alert`) |
| `discovery` | something new appeared |
| `departure` | something left |
| `alert` | attention is needed — latches until `allclear` or a host `X` |
| `allclear` | heals a latched alert |
| `tended` | someone cared for it; maintenance happened |
| `transition` | a state changed |

Ring urgency reuses the vitality scale as **tempo**: `5`–`4` breath, `3`
pulse, `2` blink, `1` strobe, `0` dark. Urgent is faster; calm is slower;
nothing is loud.

The wire format itself stays in the technical documents, where spec material
belongs: [`docs/wire-protocol.md`](docs/wire-protocol.md) carries the frame
grammar, checksums and the optional binary dialect, and
[`docs/message-inventory.md`](docs/message-inventory.md) is the complete
reference for every frame, awaiting ratification into the contract document.

## The fleet

Hardware classes live in [`hardware/classes/`](hardware/classes) — each with a
signature (how to recognize it), a manifest (what it can do), a procedure (how
to adopt it), and bench evidence. Companion names are Suzu's own: **firefly**
is the visual companion, **cricket** the audio one.

| Class | Companion | State |
| --- | --- | --- |
| Waveshare RP2040-Matrix 5×5 | firefly | **Speaks `suzu/1`** — "the lake": moments land as raindrops while atom fireflies breathe at the tempo of the host (ADR-0001). Adopted by drive copy; a census of seven units backs the signatures |
| NodeMCU ESP8266 + 0.96″ dual-zone OLED (v2) | firefly | **Live on the bench** — runs the `portrait-numerals` faceplate (beta) |
| ESP8266 OLED (v1) | firefly | Harvested ancestor firmware; no `suzu/1` face yet |
| T-Display ESP32 (ST7789) | firefly | Harvested diorama firmware with a `suzu/1` descriptor; no face rewrite yet |
| XIAO ESP32-S3 Sense | firefly | Catalogued, awaiting firmware |
| any speaker | cricket | Designed; the audio companion is still ahead of me |

Firmware comes in two planned tiers — `suzu-a` ("the ember", a minimal face
for small boards) and `suzu-d` ("the ledger", everything in
[`firmware/`](firmware)). The ancestor firmware a device ran before adoption
is kept, not deleted; `suzu restore` hands it back.

## Faceplates

A faceplate is a face's wardrobe: a package of resources, placements, and art
declared in `faceplate.yaml`. The installer pushes what the declaration lists,
verbatim — it never interprets art. One ships today:
[`portrait-numerals`](faceplates/esp8266-oled-v2/portrait-numerals), big
Bebas Neue numerals over Open Iconic status icons (OFL and MIT, licenses
included). [`tools/`](tools) carries the font-to-sprite and icon-packing
tools, plus a previewer that runs the face against a fake framebuffer.

## Where it stands

**Works today, on real hardware:**

- Identification end to end: USB descriptor ladder → catalog verdict → draft
  signature generation, proven against the bench fleet.
- The Resident: five supervised domains — host sensing (CPU, memory, GPU on
  Windows), device sessions, moment coalescing, sensor ground, publishing.
- `suzu/1` faces on two boards: the RP2040 matrix ("the lake") and the
  ESP8266 dual-zone OLED (`portrait-numerals`, live since 2026-08-28).
- In-band screenshot and GIF recording, phosphor-correct per class manifest.
- A Tauri workbench over the same loopback API (ADR-0002): the roster's
  lifecycle, the moment journal, live trail-camera panes, and the published
  card — the family system, suzu's gold. `cargo run -p suzu-workbench`
  (it lives in the tray; `serve` stays the single writer).
- Adoption with receipts: backup-first install, read-back verification, and a
  preserved `device_id` across both install paths.

**Next:**

- Ratifying the `suzu/1` wire language into `CONTRACT.md`.
- Faceplate-declared installs — today the ESP8266 push list is hardcoded and
  the matrix path copies `code.py` + `suzu.json`.
- Bench proof for the raw-REPL migration path (`suzu firmware` /
  `suzu restore` are written and verify the handshake; the migration itself is
  not yet hardware-proven).
- Cricket — the audio companion exists as design and a proven ancestor PoC.

**Not settled:**

- Transports beyond serial: SSE, web API, MCP, and stdio are specified, not
  built.
- The `suzu-a` ember tier, the YAML procedure engine, and the `suzu-fit`
  conformance suite.
- Portability: the bench is Windows; the host is plain Rust and builds with a
  stable toolchain, but only Windows has been exercised.

The design paper trail is in [`docs/`](docs) — [`the-model.md`](docs/the-model.md)
is the constitution (where older documents disagree, it wins), beside the wire
protocol, the face contract, the prior-art survey, and
[`install-lessons.md`](docs/install-lessons.md), the installation incident and
its un-brick ladder — I bricked the first board so the procedure now refuses
to. ADR-0001 records why the matrix is a lake.

## Getting started

```bash
git clone https://github.com/sylin-org/suzu
cd suzu
cargo run -- scan          # who is plugged in?
cargo run -- prepare       # adopt a face: pick a device, pick a faceplate
cargo run -- serve         # the Resident takes the night shift
```

Adopting a CircuitPython board is a drive copy with backup and verification.
Adopting an ESP8266 runs the proven pusher:

```bash
python scripts/push_firmware.py <port> <device_id> --fresh
```

The Rust path needs a stable toolchain; the ESP8266 path needs Python with
`pyserial` and `esptool`. Once a face is adopted, send it a moment by hand:

```bash
cargo run -- say completion A backup committed
```

## Kin

Suzu's companion firmware and its bench scars were harvested from the PoC that
proved them across [Zen Garden](https://github.com/sylin-org/zen-garden)'s
fleet of Stones; the names firefly and cricket are inherited from there, and
[`HARVEST.md`](HARVEST.md) is the map from this repository back to that code.
Farther kin: [Hokora](https://github.com/sylin-org/hokora) studies minds that
keep, and Nagi studies companions you hold. Suzu is the one that lets the
house feel something.

## License

To be decided — the intent is open source and community-drivable; the crate
says MIT in the meantime, and no license text ships until the choice is made.
Bundled fonts and icons keep their own licenses (Bebas Neue: SIL OFL; Open
Iconic: MIT).

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

```
 producers                 the host                      companions
 (backup, CI,        ( suzu serve — the Resident:    (a matrix on the shelf,
  Zen Garden,         senses the house,                an OLED with a face,
  an agent)      →    minds moments,             →     a bell on a speaker )
                      tends companions )
                 └───────── suzu/1 over the wire ─────────┘
```

## The contract (`suzu/1`)

The contract is transport-agnostic and versioned; if the shapes change, the
version bumps. [`CONTRACT.md`](CONTRACT.md) fixes the three message types —
the event envelope, the command manifest ("one declaration, many mouths": a
CLI generates subcommands, an MCP server generates tools, a web API generates
endpoints), and the identity handshake. The concrete wire language below is
specified in [`docs/message-inventory.md`](docs/message-inventory.md) and
[`docs/wire-protocol.md`](docs/wire-protocol.md), and is being ratified into
the contract document.

### Grounds

A companion always stands on exactly one **ground** — a state class it can
hold indefinitely. Grounds are self-healing: the host re-sends them, so a
companion that missed a frame is correct again on the next one.

| Ground | Meaning | Slots |
| --- | --- | --- |
| `report` | "Here is how the house feels" | subject, health, uptime, cpu, mem, disk, io, offerings, stones, seed_bank, hour |
| `run` | "Something is in progress" | label, progress, hue |
| `rest` | "Nothing asked for; be dark and quiet" | — |

### Data atoms and the fold law

Health and load travel as **data atoms** on a vitality scale of `0`–`5`
(5 operational … 0 offline), with off-scale `6` for plain information.
Wire form:

```
S,<set>,<axis>,<level>[,<fraction>[,<min>,<max>,<unit>]][,<text>]*hh
```

When several atoms describe one thing, they **fold**: the result is the
minimum level ≤ 5. Worst part wins. A machine with nine healthy disks and one
dying one reports the dying one — that is the atom the room needed.

### The nine rings

Where grounds are state, **rings** are moments: transient events that overlay
the ground, splash, and fade. Nine of them, each with a valence, an urgency,
and a session-scoped arc handle (`0`–`255`) so related moments can be tracked
and silenced individually.

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

### The wire

The first transport is ordinary text over serial — 115200 8N1,
newline-terminated frames, XOR checksum (`*hh`) where it matters. One frame
per idea, readable from a plain terminal:

```
I                                            # who's there?
{"proto":"suzu/1","companion":"firefly","family":"waveshare-rp2040-matrix",
 "device_id":"…","firmware":"…","pixels":25}  # the answer, within 4 seconds
G,report,12,64,3                             # ground: cpu 12, mem 64, gpu 3
R,completion,3,60,1,7,A backup committed*hh  # a ring, with a label
X                                            # silence the latched alert
```

The 4-second handshake deadline is a compile-time assert in the host, not a
comment. An optional binary dialect (`suzu-b`: COBS framing, CRC8, CBOR) is
specified for when text is too dear; the host offers it, the companion may
decline. Deliberately absent: per-frame version negotiation, timing-dependent
framing, and anything that needs a state machine to say hello.

## The CLI

One Rust binary, `suzu`. With no arguments it becomes a watch mode: polling
for hotplug, identifying what appears, offering a servicing menu.

| Command | What it does |
| --- | --- |
| `suzu scan` | Identify every serial port: USB descriptor ladder joined with the hardware catalog |
| `suzu detective` | Full fact dump per device — descriptors, probe transcript, catalog verdict, and a draft `signature.yaml` ready to paste for a new board class |
| `suzu serve` | The Resident: watcher, per-device sessions, moments, host sensing, publishing; supervised domains that announce degradation before tripping, plus a stdin door (`tell <label>`, `status`, `q`) |
| `suzu screenshot [port]` | In-band frame grab from every firefly — no reboot — written as portrait and native PNGs, colored by the class manifest's phosphor zones |
| `suzu record <secs> <fps>` | The trail camera: loops the in-band grab into a GIF, clamped to ≤ 60 s and ≤ 5 fps — the wire decides |
| `suzu prepare` | Adoption: lists CircuitPython drives and serial candidates with honest states, offers faceplates, backs up, installs, verifies |
| `suzu say <ring> [text]` | Send a moment by hand (`suzu say allclear` heals a latched alert) |
| `suzu show <tag> <text>` | Send a display string (`suzu show INFO.disk Disk at 50%`) |
| `suzu pause` / `suzu resume` | One UDP datagram to `127.0.0.1:7898` — S-U-Z-U on a phone keypad — asking the Resident to hold its breath |
| `suzu firmware <port>` | Migrate a harvested device to `suzu/1` in place: backup, push via raw REPL, soft reboot, verify identity *and* the preserved `device_id` |
| `suzu restore <port>` | Un-migrate, from the per-device backup. Refuses without one |

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

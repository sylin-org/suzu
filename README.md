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

**Part of [Sylin](https://sylin.org)** — tools that run on your hardware, show
you their work, and keep running long after you've stopped thinking about where
they came from.

## Status

Bootstrapping. The contract is designed, the PoC code to harvest is identified,
the adapters are proven on real hardware. Implementation begins after ideation
cycles confirm the surface.

See `CLAUDE.md` for the agent brief, `CONTRACT.md` for the protocol, and
`HARVEST.md` for what to read.

## The fleet so far

| Adapter | Modality | Hardware |
|---|---|---|
| firefly | visual | RP2040-Matrix 5×5, OLED v1–v2, T-Display ST7789 |
| cricket | audio | any speaker |

Your thing could be next. The contract is open; the SDK is optional.

## License

To be decided. The intent is open source, community-drivable.

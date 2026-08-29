# HANDOFF — session state for compaction (read after context compaction)

*Everything needed to continue the suzu work after context compaction.
Written 2026-08-28. Read this first; the docs it references hold the
depth.*

---

## 1 · What suzu is (one paragraph)

Suzu gives home servers a physical face: a small service (the Resident)
speaks a tiny language (suzu/1 — grounds, rings, data atoms), and
inexpensive hardware companions (fireflies = visual, crickets = audio)
render it — breathing when fine, telling when something happened,
honestly quiet when gone. Part of the sylin-org garden; harvested from
the zen-garden PoC's proven companion code.

## 2 · Where we are in the phases

- **Ideation** ✓ complete — nine design docs (see `docs/`).
- **Architecture** ✓ complete — `the-model.md` is the constitution;
  `wire-protocol.md` and `message-inventory.md` are ratification-ready.
- **Product definition** — draft done (`product-definition.md` §6 has
  three open decisions: first face = terminal vs matrix; Resident =
  always-on; contract ships internal).
- **Implementation** — STARTED. Repo `crates/suzu` has: scan /
  detective / watch verbs, the Resident with five domains talking
  (verified live: sensed → identified → minded → discovery splash →
  sensor ground), the pulse lane (audio.level stub at 5 Hz).
- **NOT started**: procedure engine in Rust (the Python
  `scripts/push_firmware.py` is the working reference), faces beyond
  the OLED translation, MCP, roster.

## 3 · The corrections (read these twice)

1. The COM12 board was never "suspect hardware". Our cancelled
   flash-write left it half-written; the USER re-provisioned it with
   the ancestor installer. Suzu made **no changes to any device** —
   all our push attempts failed harmlessly (mostly at the raw-REPL
   entry, before any write).
2. What the user saw on the OLED ("idle mode, correctly showing
   stone-leaded-sparkle with fireflies") is the OLD PoC firefly v2.0.0
   firmware running — provisioned by the ancestor installer. Not suzu.
3. Never claim success or recovery that wasn't verified by the
   handshake. Twice this session I narrated outcomes ahead of evidence.

## 4 · The next task (explicit, per the Keeper)

**Harvest `F:\Files\repo\github\sylin-org\zen-garden\installer\NewFirefly.ps1`**
(same repo as `F:\Replica\NAS\Files\repo\github\sylin-org\zen-garden\installer\NewFirefly.ps1` —
two mounts of one checkout). Learn what it does and how it does it:
device detection, variant menu (v1/v2), MicroPython flash recipe
(erase → write_flash 460800, image cached at
`~/.zen-garden/firefly-cache/`), file upload method (raw-REPL chunked
writes, 512 B), descriptor minting (GUIDv7 → zen-garden.json →
suzu.json for suzu), roster write, display test. THEN create
faceplates on top of that knowledge — starting with
`esp8266-oled-v2/portrait-numerals` (already drafted at
`faceplates/esp8266-oled-v2/portrait-numerals/main.py`, untested).

## 5 · The bench (verified facts)

- **COM12** = OLED v2 unit, CH340 (1a86:7523), hw `esp8266-05325000`,
  device_id `019d9460-4561-7196-a17d-ff53458fb039` (from tonight's
  scans). Currently runs the OLD PoC firefly v2.0.0 firmware; the user
  may have re-provisioned it via the ancestor installer (fresh
  device_id `01a04aea-aa63-7be3-995e-96fe5522eeb` was observed in an
  installer run — verify with the probe before trusting).
- The board sometimes needs the recovery ladder: Ctrl-C ×2 → Ctrl-B →
  soft reboot (`\x02\x04`) — see `crates/suzu/src/probe.rs` recovery.
- Full census: `hardware/classes/*/` — 4 classes, 15+ individuals.

## 6 · Working tools (proven tonight)

- `suzu scan` / `suzu detective` — identification ladder, catalog-
  joined verdicts (COM12 answered `esp8266-oled-v2-class · firefly
  v2.0.0` repeatedly).
- `suzu serve` — the Resident: watcher, devices, moments, sensor,
  publisher domains talking in the open; `tell <label>` visitor door;
  the OLED showed its dashboard when serve ran (ancestor translation).
- `scripts/push_firmware.py COM12 <device_id>` — the working REPL
  push: chunked escaped writes (256 B), hexlify read-back verify per
  file, soft reboot. THE reference for the Rust procedure engine.
- `python -m esptool` v5.1.0 — erase/flash verified working on this
  bench; images cached at `~/.zen-garden/firefly-cache/`.

## 7 · Key constants (bench-proven)

| Constant | Value |
|---|---|
| baud | 115200 (REPL) / 460800 (esptool flash) |
| boot wait after port open | 2500 ms (ESP auto-reset) |
| handshake deadline | 4000 ms, ask twice |
| REPL push chunk | 256 B escaped, hexlify verify in 1 KB slices |
| MemoryError floor | never alloc >2 KB on ESP8266 (80 KB heap) |
| raw REPL end marker | `\x04>` pair (never a bare `\x04`) |
| flash recipe | erase_flash → write_flash --flash-size=detect -fm dout 0x0 <bin> |

## 8 · Open questions / risks

- The Rust REPL engine (`mpush.rs`) is written but UNTESTED against
  hardware — the Python script is the proven path.
- The portrait faceplate main.py is UNTESTED on hardware (pushed once
  during a failed session; the ancestor reinstall replaced it).
- GPU capture does not exist yet — the portrait faceplate's GPU area
  will show a dash until the sensor grows the channel.
- esptool v5 syntax: use `--flash-size` / `-fm dout` (dash flags).

## 9 · Where everything lives

- Design docs: `docs/` (model → wire → inventory → catalogue →
  sharing → prior art → product definition → implementation plan →
  delight/ease → install lessons → hardware catalog/adoption).
- Firmware: `firmware/suzu-d/<board>/` (harvested, suzu/1 descriptors).
- Faceplates: `faceplates/<class>/<name>/main.py`.
- Scripts: `scripts/push_firmware.py` (working REPL push).
- Hardware manifests + census: `hardware/classes/*/`.

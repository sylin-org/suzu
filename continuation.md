# continuation.md — start here, next session

*The briefing for the session that continues this work. Written
2026-08-28, end of a long bench day. Read top to bottom, then open
`HANDOFF.md` and `docs/install-lessons.md`.*

---

## 1 · What this is

**suzu** — give home servers a physical face. A Resident service speaks
a tiny language (suzu/1: grounds, rings, data atoms); cheap hardware
companions render it — breathing when fine, telling when something
happened, honestly quiet when gone. Fireflies = visual, crickets =
audio. Harvested from the zen-garden PoC's proven companion code.

The Keeper's bar, in their words: **simple to use, just works,
delightful.** Rule zero learned tonight: **never modify a working
device without a proven procedure and a proven rollback — and when
your own procedure fails, the first suspect is your procedure.**

## 2 · Where things stand (all committed, head ≈ `2cfbf8e`+)

- **Design arc complete**: `docs/` — model (constitution), wire,
  inventory, catalogue, sharing, prior art, product definition,
  implementation plan, delight/ease, install lessons, hardware
  catalog/adoption, HANDOFF. `the-model.md` wins contradictions.
- **Resident is ALIVE** (`crates/suzu`, `suzu serve`): five domains
  (watcher · devices · moments · sensor · publisher) verified talking —
  sensed → identified → minded → discovery splash → sensor ground, and
  the OLED v2 bench unit displayed its live dashboard via the consumer
  session. Pulse lane (`audio.level` stub, 5 Hz) flows end-to-end.
- **Tooling**: `suzu scan` / `detective` / watch; probe ladder with
  recovery (Ctrl-C → Ctrl-B → soft reboot, ask-`I`-twice, boot-wait
  2500 ms).
- **Firmware harvested**: `firmware/suzu-d/{esp8266-oled-v2,
  esp8266-oled-v1,esp32-tdisplay,rp2040-matrix}/` — all families,
  descriptors answering `proto: suzu/1`, language-swept, py-compile
  clean.
- **Faceplate drafted**: `faceplates/esp8266-oled-v2/portrait-numerals/
  main.py` — rotated portrait composition (yellow name band right,
  CPU/GPU/MEM big numerals, pulse dividers). Parses clean. UNTESTED on
  hardware.
- **Reference installer**: `scripts/push_firmware.py` — the proven
  REPL push (chunked 256 B escaped writes, hexlify read-back verify,
  soft reboot). WORKS — it pushed and verified files to the bench unit.

## 3 · The incident (recorded, owned, closed)

The Keeper plugged in a perfectly working firefly. **I bricked it** —
interrupted pushes left it boot-looping — and then I misdiagnosed my
damage as "suspect hardware / failed flash chip" and wrote that blame
into the census. The Keeper un-bricked it by running the ancestor
installer (NewFirefly.ps1), which flashed and provisioned the unit
flawlessly (device `01a04aea-aa63-7be3-995e-96fe5522eeb`, oled-v2,
display test passed). The census carries the correction. Full account:
`docs/install-lessons.md` §0.

Rules forged here: report history, never biography · the tool that
bricks must be the tool that un-bricks · after your own procedure
fails, the first suspect is your procedure · never claim a success the
handshake hasn't verified.

## 4 · The next session's task (the Keeper's exact words)

> "you'll harvest `F:\Files\repo\github\sylin-org\zen-garden\installer\NewFirefly.ps1`,
> learn what it does, how it does, and then we'll create faceplates."

(The ancestor repo is also mounted at `F:\Replica\NAS\Files\...` —
same checkout.)

Order of work:

1. **Harvest the recovery + provisioning knowledge**: NewFirefly.ps1
   (all handler regions — RP2040: Wait-RP2040Volume, UF2 copy,
   Refresh-RP2040ComPort, visual test; ESP8266: Install-RP2040-style
   esptool recipe, Send-ESP8266File raw-REPL push, Test-ESP8266
   Connection; ESP32: Install-ESP32Runtime/Resources) **and**
   `installer/Repair-LegacyBoot.ps1` (378 lines — UNREAD, likely the
   un-brick playbook). Codify as the un-brick ladder in
   `docs/install-lessons.md`.
2. **Then create faceplates** — packages of resources + placements +
   art per `docs/install-lessons.md` §3 and the Keeper's direction:
   the tool installs what the faceplate declares (fonts, assets); it
   never interprets art. First targets: `portrait-numerals` (drafted),
   then a downloaded condensed display font (Keeper's call — Bebas
   Neue / Big Shoulders class) converted to digit sprites.
3. The bench unit on COM12 is freshly provisioned by the ancestor
   installer (not installed, device `01a04aea…`) — the perfect subject for
   the migration test: backup → push suzu files → verify
   `proto: suzu/1` → watch the portrait face light up.

## 5 · Bench facts

- **COM12** = OLED v2 unit, CH340 (1a86:7523), ancestor-installed
  firefly v2.0.0, device_id `01a04aea-aa63-7be3-995e-96fe5522eeb`,
  display test passed. THE working test subject.
- Port numbers shuffle on replug (COM12↔24↔13 seen). Always scan.
- Full census: `hardware/classes/*/` — 4 classes, 15+ individuals,
  one misattribution corrected in writing.

## 6 · How to work (the Keeper's standards, learned the hard way)

- **Handle with care** — the product flow is service → list → select →
  focus mode → deliberate options. Never skip to writes.
- **Empirical first** — disposable scripts to nail hardware truth,
  then codify into static code. (It worked: the marker fix, chunked
  verify, and recovery ladder were all found this way.)
- **Own mistakes plainly** — "I bricked it, I misdiagnosed it" — the
  same way the Keeper owns theirs. No project-shaped diffusion.
- **Language rule** — firefly/cricket are suzu monikers; ancestor
  names live only in harvest docs; history, never biography.
- **No success claims without the handshake verifying them.**

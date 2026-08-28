# Install lessons — what the bench taught the automated installer

*Every failure, quirk, and constant from the hands-on sessions of
2026-08-28, structured for the procedure engine and the adoption tool.
Companion to [`hardware-catalog-and-adoption.md`](hardware-catalog-and-adoption.md)
and [`implementation-plan.md`](implementation-plan.md) §3.*

---

## 0 · Current state (open item)

COM24 (OLED v2, device_id `019d9460…`) is mid-recovery: flash was
erased and one re-flash attempt was interrupted. Next session: finish
with the ancestor recipe — `esptool erase_flash` then
`esptool --baud 460800 write_flash --flash_size=detect 0x0
micropython-esp8266.bin` (cached at `~/.zen-garden/firefly-cache/`) —
then REPL-push the suzu folder (`scripts/push_firmware.py` already
does the push + verify end-to-end) and confirm `suzu scan` answers
`proto: suzu/1` with the preserved device_id.

## 1 · Sense & identify

| Lesson | Evidence | Rule |
|---|---|---|
| **Ports are not identity** | CH340 bridges report a constant serial (`6`) — two different units are USB-twins | Never key state on port names; always re-probe. Class notes carry the warning |
| **Bridge ≠ board** | CH9102 sits on many boards; CH340 on many others | VID/PID is a *hint*; the probe (`I`, 4 s) is the authority |
| **Boot noise glues to responses** | `HOK,{…}` and `¿���OK,{…}` seen on two families | Parsers extract JSON from first `{` to last `}`; never anchor-prefix |
| **Boot races swallow the first `I`** | Probe sent `I` at 1.5 s; ESP8266 was still booting; no answer | Ask twice (4 s deadlines), after a full 2.5 s boot wait |
| **Non-USB ports are foreign by default** | COM1 and 12 Bluetooth/virtual ports on the bench machine | No USB descriptor → skip silently (one line in the log) |
| **A probe can murder the thing it recovers** | Recovery sent `\x03\x03` *after* the soft reboot → KeyboardInterrupt killed the app that had just come back | Recovery = `\x02\x04` + wait, **never** an interrupt afterwards |

## 2 · Mind (the handoff)

| Lesson | Evidence | Rule |
|---|---|---|
| **Report-before-minding** | First migration attempt got "Access denied" (the port was held) — the watcher must not mind a device it could not read | Unreachable/busy ports are `PortBusy` facts for the house; only honestly-read devices become `Device` entities |
| **Channel doors go stale** | The supervised devices loop recreated its command channel after the watcher cloned the old sender — the first `mind()` vanished silently | Doors are created once before spawning; supervised loops recreate them **only on restart, and re-wire the house at that moment** |
| **Dropped futures are lost messages** | `drop(reply.send(...))` on a tokio bounded channel never sent | `.await` every send; the snapshot bug was found by the bench, not by review |
| **The cold-prime lie** | First CPU sample reads 100% | Prime the sensor with a real interval; discard the first sample |

## 3 · Flash (the bootloader path)

| Lesson | Evidence | Rule |
|---|---|---|
| **The ancestor recipe is exact** | `erase_flash` + `write_flash --baud 460800 --flash_size=detect 0x0 micropython-esp8266.bin` — omitting `--flash_size=detect` and the 460800 baud preceded a boot-loop state | Codify verbatim as the procedure's flash step; no "improvements" |
| **MicroPython images are already on the bench** | `~/.zen-garden/firefly-cache/micropython-esp8266.bin`, `micropython-esp32-st7789.bin`, CircuitPython UF2s, `neopixel.mpy` | Install works offline; the cache directory is the image source |
| **esptool v5.1.0 works from Python** | chip_id, erase_flash, write_flash, hash verify all succeeded | The flash backend can shell out to `python -m esptool`; ROM bootloader answers even from a crash loop |
| **Crash-loop signature** | Continuous binary garbage at 115200 = boot spew at 74880 misread; board reboots faster than we read | Diagnose by listening at 74880; treat as "unreachable — recovery path", never as fresh |
| **Backup-first is what made failure harmless** | The failed migration attempt wrote nothing (it stopped at `list_files`) — because backup precedes every write | Any procedure step that writes must be preceded by a verified read of what it replaces |

## 4 · REPL push (the gentle path)

| Lesson | Evidence | Rule |
|---|---|---|
| **Ctrl-D in the friendly REPL = soft reboot** | The first push sent code + Ctrl-D at the friendly prompt; the board rebooted mid-backup | `exec` must ensure raw mode (Ctrl-A) *before* code; track the raw flag; a timeout means framing is unknown → drop to unraw and re-interrupt next call |
| **Chunk size is a RAM ceiling** | ESP8266 has 80 KB; the ancestor used 512 B chunked raw-REPL writes and called mpremote's `cp` "flaky" | 256 B chunks, read-back verified (the ancestor's `Test-FireflyBootPy` pattern) |
| **Interrupt wakes the prompt** | `\r\x03\x03` + 700 ms drain before any REPL session | The app catches KeyboardInterrupt and yields the prompt — by design |
| **Re-plug ≠ state loss** | Homecomings re-identified by device_id across the census | The roster, not the port, remembers |

## 5 · Verify (the loving part)

- Read-back after **every** file write (`cat` the file, compare bytes).
- The final step of every procedure is the contract handshake: the
  device must answer `I` with `proto: suzu/1` **and the same
  device_id** it had before. Identity is preserved across migration;
  the verify proves it.
- The OLED census units that answered `firefly v2.0.0` pre-migration
  must answer `firefly v2.0.0 [suzu/1]` post-migration — same faces,
  new language.

## 6 · Procedural constants (codified from the bench)

| Constant | Value | Source |
|---|---|---|
| Boot wait after port open | 2500 ms | ESP auto-reset on open (harvest) |
| Handshake deadline | 4000 ms | compile-time assert, carried from the PoC |
| Handshake retries | 2 asks | boot-race tolerance (this session) |
| REPL interrupt | Ctrl-C ×2 + 700 ms drain | ancestor `Send-ESP8266File` |
| REPL push chunk | 256 B | ancestor used 512 B; halved with margin |
| REPL exec timeout | 15 s | ancestor read timeout 10 s + margin |
| Flash baud | 460800 | ancestor recipe |
| Recovery soft reboot | `\x02\x04` + 2500 ms | this session |
| Ground cadence | ~2 s on drift | resident design |
| Pulse cadence | ~5 Hz lean frames | pulse-lane design |

## 7 · For the adoption UX

- A **busy port is a message, not an error**: "COM24 is held by another
  program — close it and I'll try again."
- A **silent port is a diagnosis, not a shrug**: "no answer — try a
  data cable, or hold BOOT while plugging to force the bootloader."
- The list shows **what the tool knows**: fresh / pre-suzu / suzu x.y /
  busy / unknown — never a guess dressed as a fact.
- After any write: verify, then *say so* — "wrote and verified" beats
  "done".

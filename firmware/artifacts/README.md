# Vendored runtime artifacts

Factory reset and factory-fresh onboarding work offline, so every runtime
image the procedures need is kept in this directory, checksummed, and
validated before a device is erased. Each artifact records its provenance
and its consumer; an artifact nobody consumes is deleted, not accumulated.

| File | Consumer | Role |
|---|---|---|
| `circuitpython-raspberry_pi_pico.uf2` | Resident — `install` / `factory` for waveshare-rp2040-matrix | CircuitPython runtime, copied over BOOTSEL drive |
| `flash_nuke.uf2` | Resident — `factory` for waveshare-rp2040-matrix | Flash erase image, copied over BOOTSEL drive |
| `micropython-esp8266-1mib.bin` | Resident — `install` / `factory` for esp8266-oled (native ROM bootloader) | MicroPython runtime, written at offset 0x0 |

The ESP8266 runtime is flashed through the native ROM bootloader
(crates/suzu/src/bootloader.rs): before writing, the chip's JEDEC ID is read
and the image header's flash-size field is set to match — the equivalent of
the ancestor recipe's `--flash_size=detect`, without which a 1 MiB image on a
larger chip boot-loops (install-lessons.md §3).

Provenance and sha256 for each artifact are recorded in the owning class's
`procedure.yaml` (`artifacts:` section). The first byte of an ESP image must
be the magic `0xE9` — the flasher refuses an artifact that is not, before
the board is touched.

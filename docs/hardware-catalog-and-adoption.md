# The hardware catalog & adoption

*Plug, identify, adopt: a known-hardware catalog and an OS-agnostic
adoption tool.*

Proposal date: 2026-08-28. Under [`implementation-plan.md`](implementation-plan.md)
(milestones) and [`delight-and-ease.md`](delight-and-ease.md) (the
10-minute test this serves).

---

## 1 · The proposal

Plug in several devices — one of each kind and capability — and learn to
identify them by their **signature**. The outcome is a **known-hardware
catalog** (declarative, versioned, community-grown) feeding an
**OS-agnostic Rust tool**: the user plugs, the tool identifies
("device type A, fresh", "device type B, suzu firmware 1.0"), and offers
what can be done — install/upgrade suzu, install sets — *according to
device limitations*, memorialized from prior observation in the catalog
and coverage manifests.

This is `suzu adopt` made real — the adoption ceremony from the use
cases, and the PoC's provisioning ritual (NewFirefly) generalized from
a PowerShell script into a product.

## 2 · The identification state machine

Every plugged serial device walks one ladder, sniff-first, probe-second,
bootloader-last:

```
plugged
 → passive window: unsolicited * HELLO frame (suzu descriptor)?
    → suzu <version>, coverage class   [done — no flashing]
 → `I` probe (4 s deadline, the contract handshake)
    → suzu descriptor?                 → suzu <version>
    → zen-garden.json descriptor?      → legacy fleet (offer migration)
 → descriptors heuristics: CircuitPython/MicroPython strings,
   VID/PID + product patterns against the catalog
    → known-fresh (board recognized, no suzu firmware)
    → unknown ("looks like a board we know — confirm?")
 → bootloader evidence: UF2 mount (RP2040), esptool chip id (ESP32)
    → known-fresh (bootloader path)
 → nothing: foreign serial device — leave it alone
```

Verdicts, exactly as the user phrased them: *fresh*, *suzu firmware
x.y*, plus three the harvest demands: **legacy fleet** (24 minted
fireflies running firefly-fw 0.2.0/1.0.0 must migrate — recognizing
one's ancestors and carrying them forward is a delight beat, not a
migration chore), **unknown** (honest ambiguity: propose a catalog
match, never guess), **foreign** (never touched).

## 3 · The catalog

YAML, versioned, community-grown — the tune pattern applied to
hardware. Shipped core, overlay-able from the filesystem; PRs grow it
by adoption, not decree (same law as sets).

```yaml
- match:
    vid_pid: ["2e8a:000a", "2e8a:0003"]     # patterns, not singletons
    product_regex: "RP2040|Pico"
  board: waveshare-rp2040-matrix
  family: rp2040-matrix
  coverage: suzu-d            # + frames capability
  flash:
    backend: uf2
    procedures:
      install: rp2040-bootsel     # named procedures — see §4
      upgrade: cp-drive-copy
      recover: rp2040-bootsel
  firmware:
    suzu-a: null              # not offered for this board
    suzu-d: firmware/suzu-d/rp2040-matrix.uf2
  sets: [os]                  # recommended, per limitations
  constraints: "25 px; host-side frames engine"
  notes: "boot sweep ~1 s; brightness 0.3 default"
```

The catalog is where the harvest's tribal knowledge is **memorialized**:
ESP boot waits (2.5 s), CH340-vs-native-USB ambiguity, ESP8266 RAM
ceilings (→ suzu-a/d limits), per-board flash methods, set capacity.
The manifests (coverage) carry what a *running* device can do; the
catalog carries what a *board* can host. The tool joins them.

## 4 · Flash procedures — the file drives the user experience

Every flashing mechanism has *characteristics*: what to copy, what to
hold, what to watch, how long to wait. These are codified per device as
a **procedure** — an ordered checklist of steps from a **closed
vocabulary**. The tool is a generic step engine; adding device #26 is
writing YAML, not code.

### The step vocabulary (closed, reviewed, sandboxed)

| Step | Does | Watched by |
|---|---|---|
| `say` | shows a sentence to the user | — (pure instruction) |
| `ask` | prompts; continues on Enter | user confirmation |
| `wait-mount` | waits for a volume (RPI-RP2, CIRCUITPY…) to appear/disappear | OS mount events |
| `wait-port` | waits for a serial port matching vid/pid/regex to appear/disappear | re-enumeration |
| `copy` | writes file(s) to a mount | file system result |
| `open-serial` | opens a port at a baud rate | port result |
| `repl-raw` / `push` / `soft-reset` | MicroPython remote-REPL file push | echo/ack |
| `dtr-rts` | asserts modem lines (auto-reset bootloaders) | port behaviour |
| `backend` | invokes an allowlisted tool (espflash…) with template args | exit code |
| `sleep` | fixed wait — the harvest's numbers live here (ESP boot: 2.5 s) | timer |
| `probe` | runs the contract handshake, expects a coverage class | identity response |

Every step carries `timeout` and `on-timeout` (the retry sentence).
**The rule that makes it trustworthy: every human action is followed by
an observable** — a mount appearing, a port re-enumerating, a probe
answering. The tool never says "press Enter and hope"; it watches the
bus to confirm each physical step actually happened.

### Procedures are checklists, not a language

No loops, no conditionals beyond one `on-timeout` hint, no arbitrary
code (`backend` is restricted to allowlisted tools with template
args). This keeps entries reviewable, translatable, and safe — a
community PR that flashes the wrong thing is visible as prose.

### Examples

**RP2040 fresh — the BOOTSEL dance:**

```yaml
rp2040-bootsel:
  - say: "Hold the BOOT button on the board."
  - say: "Keep holding it while you plug the USB cable in."
  - wait-mount: { label: RPI-RP2, timeout: 30s,
      on-timeout: "No RPI-RP2 drive appeared — unplug and retry,
                   holding BOOT firmly." }
  - copy: { file: suzu-d-rp2040-matrix.uf2 }
  - wait-unmount: { label: RPI-RP2, timeout: 15s }
  - sleep: 2s
  - probe: { expect: suzu-d, timeout: 4s }
```

**ESP8266 upgrade — REPL push (the harvest's numbers, codified):**

```yaml
esp8266-upgrade:
  - open-serial: { baud: 115200 }
  - sleep: 2.5s                 # ESP auto-reset on open
  - repl-raw: {}
  - push: { files: [boot.py, main.py, suzu_oled.py] }
  - soft-reset: {}
  - sleep: 2.5s                 # boot wait
  - probe: { expect: suzu-d, timeout: 4s }
```

**The "copy, then hold RESET" pattern** (boards whose bootloader is
reached by a held reset after mounting, or by double-reset):

```yaml
copy-hold-reset:
  - wait-mount: { label: "{mount}", timeout: 20s }
  - copy: { file: "{firmware}" }
  - say: "Press and hold the RESET button for {n} seconds."
  - wait-port: { vid_pid: "{boot_vid_pid}", timeout: 20s }
  - probe: { expect: "{coverage}", timeout: 4s }
```

Three procedures cover the known zoo — UF2/BOOTSEL, drive-copy
(CircuitPython upgrades), and serial bootloaders — and the vocabulary
exists so the *fourth* kind of board needs only a fourth checklist.
Procedures per lifecycle: `install` (fresh), `upgrade` (already
speaks suzu — usually the gentle drive-copy), `recover`
(half-flashed; the bootloader path).

## 5 · The tool: `suzu adopt`

```
$ suzu adopt
scanning serial devices…
  /dev/ttyUSB0   suzu firmware 0.3 (suzu-d)     ok — up to date
  /dev/ttyACM0   device type A, fresh           install suzu-a? [Y/n]
  /dev/ttyUSB1   legacy firefly (fw 0.2.0)      migrate to suzu? [Y/n]
  /dev/ttyUSB3   unknown serial (1a86:7523)     not in catalog — skipped
name the new device: gentle-ember
hue: derived from name → 214°
first breath… ✓
```

Flashing is destructive, so the ceremony has **safety laws**:

1. **Never touch foreign devices** — the foreign verdict is a wall.
2. **No flash without consent** — show board, tier, and what erases.
3. **Verify after write** — read-back + descriptor probe before
   claiming success (the PoC's known gap: one-way blind copy).
4. **Unplug-safe** — the registry's re-probe backoff covers mid-flash
   yanks; a half-flashed device reappears as bootloader evidence, and
   the tool offers to finish.

## 6 · Flashing backends — honest assessment

| Backend | Boards | OS-agnostic? | Notes |
|---|---|---|---|
| **UF2 drag/drop** | RP2040 | yes — it's a file copy to a mounted drive | the easy path; detect the mount, write, done |
| **espflash (Rust crate)** | ESP32 family | yes | official Espressif Rust tooling |
| **serial REPL push** | MicroPython boards (incl. ESP8266 file systems) | yes — protocol is simple (raw/paste REPL) | the mpremote logic, portable; known work |
| **esptool lineage** | ESP8266 boot images | via crate support — *unverified* | risk item: if no Rust path, ESP8266 starts as "advanced path" |

Risk register: ESP8266 boot-image flashing is the one backend without a
certain Rust story. Mitigation: v1 ships UF2 + ESP32 + REPL push; the
ESP8266 boot path follows (or stays manual) without blocking the
product — REPL push covers the MicroPython-on-ESP8266 case, which is
the PoC fleet's own shape.

## 7 · Sets at adoption

Set recommendations come from the catalog entry (`sets: [os]`) filtered
by the coverage class the board can host — limitations memorialized
ahead of the event, exactly as proposed. Adoption writes the fluency
into the descriptor; the host learns it at the handshake. The ember
installs nothing (it is folded); the ledger takes `os`; the diorama
takes `os` + `storage`.

## 8 · Where it sits

- **v0 shipped** (`crates/suzu-adm`): `scan` (one-shot harvest) and
  watch mode; identification ladder with non-USB/foreign-by-default
  classification. **First observation 2026-08-28, COM12**: a legacy
  fleet member — firefly v2.0.0 OLED-v2 (dashboard, dual-zone 128×64,
  GUIDv7 `019d8d7a…`, hw `esp8266-26953e00`) — answered `I` on demand
  with its full descriptor. Grounded in
  `hardware/catalog.yaml`; migration target in
  `hardware/descriptors/suzu-device.template.json`.
- **M2.5**: `suzu adopt` + catalog + the identification ladder,
  validated against a virtual device.

- **M2.5** in the implementation plan: `suzu adopt` + catalog + the
  identification ladder, validated against a virtual device.
- **M3**: validated against real hardware, one board per class — the
  user's proposal is literally the M3 test protocol: plug one of each
  kind, learn the signatures, write the catalog entries.
- Feeds **suzu-fit**: a catalog entry maps to expected fixtures, so a
  known board is certified against its class automatically.

The catalog grows by adoption — every new board someone plugs and
documents makes the next person's ten minutes shorter.

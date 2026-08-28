# The wire protocol

*Draft: suzu/1 on the serial hop — framing, encoding, reliability classes —
under the asymmetry principle.*

Draft date: 2026-08-28. Under [`the-model.md`](the-model.md). Prior-art
basis: COBS+CRC embedded practice, MAVLink, NMEA 0183, Modbus RTU
(anti-pattern), CBOR, MQTT QoS — sources in the chat trail and §7.

---

## 1 · The asymmetry principle

**The host always has more firepower than the companions.** Every protocol
decision is made by asking one question: *who does the work?* The answer
is almost always the host, because on the constrained side, protocol
weight is paid for out of the **face budget** — the firmware's flash, RAM,
and CPU are the repertoire's budget. The ESP8266 firmware literally
deleted features ("scroll, spinner, slide — to save memory") to make room.
A protocol that demanded COBS+CBOR+CRC tables from every companion would
be spending face animations to buy encoding cleverness. The host has
megabytes; the cleverness lives there.

The most proven architecture in computing is built on exactly this
asymmetry: **USB**. The host polls, schedules, adapts, and recovers; the
device offers simple descriptors and simple endpoints. Suzu's shape is the
same: identity + manifest are descriptors; slots and faces are endpoints.

## 2 · The two charters

**The host does the work:**

- negotiate profile and version at the session (never per packet);
- translate, diff, coalesce, and rate-shape — the companion receives
  *fewer, smaller, smarter bytes*, not denser dialects;
- own all the ugly: boot noise, partial lines, garbage, re-probe, retry,
  legacy detection, tolerance;
- enforce budgets and tenancy (the mixer);
- remember everything (state, arcs, fallback chains, kind tables).

**The companion stays delightfully dumb:**

- declare itself (identity + coverage) — one JSON line on boot, answer
  `I`;
- receive slots, run faces, report truth (`OK`/`ERR`);
- drop what doesn't parse — one bad line costs one line, never the
  stream;
- keep breathing when the host goes quiet.

## 3 · Profile A — `suzu-t` (text, mandatory)

The PoC's line protocol, hardened. Chosen because it is the
**companion-cheapest** encoding on *both* axes that matter: code size
(strstr/strtol, no codec libraries, fits the ESP8266 today) and wire
bytes (within ~30% of optimal binary for suzu's small-integer value
profile — single digits per message).

### Frame grammar

```
<opcode>[,<field>]*[*<xor-hex>]\n
```

- `\n` terminates; receivers buffer lines with a max-length guard
  (overflow → drop the line, resync at next `\n`).
- Opcode-first; each opcode has a declared **arity + field-type table**.
  Unknown opcode or arity mismatch → drop silently. Grammar is the first
  integrity check — boot spew cannot accidentally parse.
- `*hh` XOR checksum (NMEA-style, over bytes before the `*`):
  **required** on lines that mutate session state (identity, config,
  scene bindings); optional on the hot path, where arity + range
  validation already catches most corruption.
- Strings occupy final slots only, comma-free; anything richer goes
  through the JSON escape.

### The kinds (draft starter — the message inventory remains open work)

| Kind | Form | Class | Example |
|---|---|---|---|
| identity | `I` | session | `I\n` → `OK,{...}\n` |
| help | `?` | session | `?\n` → `OK,G,D,J,R,P,F,C,B,A,S,I,?\n` |
| ground.set | `G,<scene>,<slots...>` | state | `G,report,42,61,30\n` |
| ground.delta | `D,<slot-index:value...>` | state | `D,1:44,3:12\n` |
| json escape | `J,{...}\n` | state | full snapshot with complex values |
| ring | `R,<signal>,<tempo>,<hue>,<label...>` | moment | `R,alert,2,24,redis*5a\n` |
| restore | `X` | state | end of ring overlay, ground resumes |
| frames | `P,x,y,r,g,b` · `F,r,g,b` · `C` | state | frame-capable devices only |
| ack | `OK[,...]` / `ERR,<reason>` | session | every line acknowledged |

Continuity note: this grammar is a **superset of the PoC wire protocol**
(`D`/`L`/`T`/`J`/`P`/`F`/`C`/`B`/`A`/`S`/`I`) — the 24 minted devices are
nearly suzu-t compliant today; the additions are the checksum, the ring
kind, and the arity tables.

### Context lives in the session

Version, source attribution, hue assignments, arcs, and ground context
are negotiated once at the handshake. A packet carries only **boundary,
kind, payload, integrity** — the minimal functional information.

## 4 · Profile B — `suzu-b` (binary, optional, host-offered)

For devices where bytes genuinely bind (frame pushing: 30 fps × 25 pixels
of `P` lines ≈ 12.7 KB/s against an 11.5 KB/s budget). Offered by the
host at handshake; **opted into** by the companion declaring `"enc":
"cobs"`.

```
0x00 | kind(1) | payload | crc8 | 0x00      COBS-framed, CRC-checked
```

- Same kind table as suzu-t; CBOR payload when values are complex.
- MAVLink lessons: stable kind IDs; sequence byte on rings for loss
  visibility; version is session-scoped, never per packet.
- Before offering it, the host must first exhaust its own firepower:
  **differential updates** (touch only changed pixels — a few `P` lines
  per frame), coalescing to the scene budget, quantizing. Denser
  encoding is the *last* resort, not the first, and the companion never
  implements complexity it cannot afford.

## 5 · Reliability classes

| Class | Kinds | Guarantee | Mechanism |
|---|---|---|---|
| **state** | ground.set, delta, frames, restore | self-healing, at-most-once | idempotent; next delta repairs any loss (MQTT retained-message semantics) |
| **moment** | ring | at-least-detectable | sequence echo on ack; host re-sends once in window; idempotent by (arc, phase, seq) |
| **session** | identity, help, config | acknowledged + checksummed | the 4-second handshake, probe retry, `*hh` required |

Rationale: most traffic is state, and state does not want acknowledgments
— it wants *the truth, eventually, cheaply*. Moments are few and worth a
sequence byte. Sessions are rare and worth ceremony.

## 6 · Anti-patterns (rejected by prior art)

- **Timing-based framing** (Modbus RTU's silence gaps) — breaks behind
  buffered USB-CDC stacks; disqualified for this host path.
- **Per-packet version fields** — session-scoped versioning only
  (MAVLink's lesson, minus the per-packet tax).
- **Binary as the mandatory core** — taxes the face budget of every
  companion to save bytes that only one device class needs.
- **Rich companion-side state machines** — sync-scan, negotiation,
  retry logic live host-side; the companion's receiver is a line buffer
  and a grammar check.
- **Unbounded lines** — every profile carries a max-length guard;
  overflow is a drop, not a hang.

## 7 · Prior-art sources

- [COBS — Wikipedia](https://en.wikipedia.org/wiki/Consistent_Overhead_Byte_Stuffing);
  [framing deep-dive](https://www.embeddedrelated.com/showarticle/113.php);
  [practitioner survey](https://www.reddit.com/r/embedded/comments/11bxeux/whats_everyone_is_using_for_framing_and/)
- [MAVLink packet serialization](https://mavlink.io/en/guide/serialization.html)
- [NMEA 0183 overview + XOR checksum](https://receiverhelp.trimble.com/alloy-gnss/en-us/NMEA-0183messages_MessageOverview.html);
  [framing/parser separation](https://docs.rs/nmea0183-parser)
- [Modbus RTU silence framing](https://industrialmonitordirect.com/blogs/knowledgebase/modbus-rtu-framing-silent-interval-and-crc-16-in-plcs)
- [CBOR vs the other guys](https://cborbook.com/introduction/cbor_vs_the_other_guys.html);
  [CBOR for IoT payloads](https://hubble.com/community/comparisons/json-vs-cbor-for-iot-payloads-which-encoding-when-every-byte-counts/)

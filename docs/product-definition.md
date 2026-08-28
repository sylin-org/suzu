# Product definition — DRAFT for ratification

*What suzu is as a product: for whom, the promise, the experience, what
ships, and what acceptance means. Architecture (the-model, wire,
inventory) serves this definition; this document outranks them on
product questions.*

Draft: 2026-08-28. Status: **draft — the product decisions in §6 are
open.**

---

## 1 · The promise

**Give your server a face.**

One sentence a stranger understands: *your machine becomes a small
presence in your home — it breathes when it's fine, tells you when
something happened, and goes quiet — honestly quiet — when it's gone.*

The promise is falsifiable. The product delivers when the four moments
(§3) work for a person who never reads a design document.

## 2 · Who it is for

| Persona | Role | Relationship to the product |
|---|---|---|
| **The Keeper** | has a server (repurposed e-waste, old laptop, second gaming rig) | the customer. Buys nothing; adopts it. |
| **The Household** | shares the home | zero-effort beneficiary. Sees the face, learns nothing. |
| **The Maker** | builds companions | supply side. Served by the contract and catalog, later. |
| **The Producer** | apps that want a voice | supply side. One webhook/curl, later. |
| **Agents** | AI assistants | post-1.0. Scoped, polite actuation. |

v0's customer is **the Keeper alone**. Everyone else is why the
architecture is shaped the way it is — not who v0 is for.

## 3 · The experience (the four moments)

1. **Pairing** — plug in a device (or none), give it a name, watch the
   first breath. From zero to breathing in minutes, with at most one
   page of instructions, and honest messages when a cable lies.
2. **Living presence** — the face at rest: breathing, present,
   glanceable. It never lies, never nags, and drifts quietly with the
   machine's truth. **The ground needs no users**: the Resident ships
   with a built-in per-OS environment sensor (name, cpu, mem, disk,
   uptime) as its default ground source. Service running + device
   plugged in = the full ambient display, with nothing else installed
   and nothing configured.
3. **The visit** — when a *user* exists (an agent over MCP, zen
   garden, a script, a webhook) and it issues a moment, the face
   **splashes it for a few seconds, then returns** to the ambient
   ground. Bursts coalesce rather than stutter; a splash never clears
   a latched alert; the return is always quiet. Visitors are garnish —
   silence between visits is the product working.
4. **The absence** — when the machine dies, the room notices without
   an alarm. When it returns, the return is felt. No false absences
   across reboots and cable swaps.

These four are the product. Anything that doesn't serve one of them is
architecture or ecosystem, not surface.

## 4 · What the product is made of

Derived from the experience, not from the architecture:

- **The Resident** — an always-on service on the server (decided
  2026-08-28); includes the built-in environment sensor and the loopback
  door for visitors; the thing whose death is legible. One binary:
  sensor, ground, splash mixer, companion sessions.
- **The Face** — one companion that renders the ground and rings:
  a terminal face (zero hardware) and/or device firmware.
- **The Adoption** — `suzu adopt` / `suzu detective`: pairing and
  servicing (v0.1 exists).
- **The Name** — identity, hue, roster; the memory that makes
  homecomings warm.

Explicitly **not** product surface: the contract, the wire format, the
manifest catalog, the census. They are the supply chain that makes
companions cheap — architecture, documented elsewhere, invisible here.

**One binary** (decided 2026-08-28): everything ships as a single
executable. Verbs are doors, not products:

```
suzu                    run the Resident (foreground; `suzu service install`
                         registers it as an OS service)
suzu adopt              detection, pairing, firmware install/upgrade
suzu detective          the fact dump
suzu face               the terminal face (also embedded in the Resident)
suzu tell <moment>      ring the bell yourself — the splash, from your hand
```

Reference firmware and calm defaults are embedded; `hardware/` and
`tunes/` folders overlay them (the tune pattern). The exe is
self-sufficient: one file copied to any machine is the whole product.

## 5 · v0 scope

**In:** one Keeper, one server, one face; pairing; the four moments;
the built-in environment sensor as the default ground (zero config);
the minimal ring set (heartbeat, completion, discovery, alert,
all-clear — a visitor's INFO rides `transition` with a label); the
report ground; absence detection; the loopback door for visitors.

**Out (post-v0):** multiple rooms and flocks; agents and MCP; sets
management UI; budgets UI; community catalog flows; cross-stone
anything.

## 6 · The open product decisions (needs your call)

1. **v0's first face — terminal or matrix?**
   *Terminal-first* reaches "just works" fastest (zero hardware, no
   flashing) and makes the product testable by anyone. *Matrix-first*
   is maximum delight and uses the bench, but inherits the flashing
   cliff. Recommendation: **terminal is the product's face; the matrix
   is the demo** — both get built, only one is the default path.
2. **The Resident's shape** — always-on daemon (required for absence
   detection and rings while you're away) vs run-on-demand. 
   Recommendation: **always-on service, zero config**, because absence
   is a product moment and needs a witness.
3. **Does the contract ship in v0?** As a public spec (inviting
   companions early) or internal (one face, one resident, no outside)?
   Recommendation: **internal for v0** — one face, one resident, no
   outside until the four moments are solid.

## 7 · Acceptance — derived from the product, not the implementation

Acceptance passes when these are *experienced*, not when code exists:

- **A1 · Fresh pairing** — a person who has never seen suzu reaches
  first breath using only what ships. One page. Honest failures.
- **A2 · Truthful presence** — a week of daily life: the face never
  lies, never nags, drifts with reality.
- **A3 · Legible absence** — a human notices a dead server in seconds
  without an alarm, and routine reboots never fake an absence.
- **A4 · The guest moment** — someone who isn't the Keeper asks about
  the face, unprompted.

The earlier "six tests" (60-second, 10-minute, glance, absence, guest,
weekend) remain as *experience metrics* beneath these — but they hang
from this document, not from any implementation plan. They may be
re-measured only after §6 is decided and the slice exists.

## 8 · Product phases

- **P0 — one face.** One Keeper, one server, one companion. The four
  moments, end to end, honestly.
- **P1 — the household.** More machines, more faces, rooms and flocks,
  producers beyond self-report.
- **P2 — the valley.** Agents, community companions, the contract as a
  public thing.

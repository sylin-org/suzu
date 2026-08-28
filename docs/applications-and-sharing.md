# Applications & sharing

*How applications use suzu — the four roles, evidenced by the garden's PoC —
and how many applications share one bell.*

Design date: 2026-08-28. Companion analyses:
[`poc-companion-surface.md`](poc-companion-surface.md),
[`prior-art-and-positioning.md`](prior-art-and-positioning.md),
[`presence-catalogue.md`](presence-catalogue.md).

---

## 1 · The four roles

The PoC had exactly one producer, and it used the companion surface in four
distinct roles. Suzu's five transports are these roles made official: the
same command manifest serves all of them.

| Role | Transport | Who | What they do |
|---|---|---|---|
| **producer** | SSE (pull) or push | software with a story | emit envelopes into the shared vocabulary |
| **client** | vesper CLI, web API | humans, tests, scripts | touch the companion directly (`pixel`, `play`, `tend`) |
| **agent** | MCP | AI assistants | converse through manifest-derived tools, scoped per companion |
| **embedder** | stdio | scripts, other processes | line-delimited JSON for tight coupling |

### 1.1 Producer — the bridge pattern

Moss never thought about companions. Internally it emitted *domain events*
(offering, stone, storage, job, pond variants) on its own event bus. One
translator module — the pulse listener
(`moss/src/infra/listeners/pulse.rs`) — converted them into the presence
vocabulary (`from_offering`, `from_storage`, `from_stone`) and published
into a single broadcast channel feeding the SSE stream. Companions
subscribed and pulled, with backoff.

The crucial property: **the application speaks its own domain language, and
one bridge translates it into shared signals.** The heal-moment vocabulary
(ADR-0006) was the design of that translation table — and it was editorial
work, not plumbing: "health-degraded/healed don't exist as events yet; they
emerge from the converge loop's transitions." A producer's real
contribution is choosing which internal moments deserve a ring.

What the garden actually said: stone topology (`seen`/`goodbye`/`expired`
plus load deltas and health changes), offering lifecycle
(`planted`/`rested`/`woke`/`uprooted`), capture milestones (`committed`,
with `run_id` and `final_hash`), storage (`connected`/`detected`/`removed`),
tending (`by`, `from`, a human message), job progress. The granularity
lesson: **snapshots for grounding, deltas for drift, discrete milestones
for rings** — and small semantic payloads (`fqn`, `stone`, `hash`), not
dumps.

### 1.2 Client — the command path

`hey tell firefly pixel 2 2 ff0000` → the host proxies to the companion's
loopback server (auto-starting it if resting, 5 s timeout) → the command
becomes a `CommandInvocation` on the companion's internal bus → the
adapter executes and a correlated `CommandResponse` returns. `hey tell
firefly all …` fans out topology-wide. Humans drove companions this way
(volume, on/off, tend); the test harness drove them this way too (probe
e2e made cricket play). Tests-as-clients is a quietly great pattern worth
keeping.

### 1.3 Agent and embedder

Designed, not yet built: MCP tools derived from the manifest, one
companion at a time, loopback-scoped — the polite actuation surface for
agents. stdio JSON for embedding and scripting.

## 2 · Producer adoption models

Two legal patterns; the contract should bless both:

- **Pull (resident producers).** The companion subscribes to the
  producer's stream — the garden model. Right for long-lived producers
  with continuous state (a server's self-report).
- **Push (drive-by producers).** A CI webhook, a backup script, a cron
  job POSTs one envelope and vanishes. Right for episodic producers —
  and it is where producer zero lives: an application adopts suzu by
  sending **one JSON object to one URL**.

## 3 · Sharing one bell

One producer made sharing trivial: one kind namespace, one tune YAML.
Multi-producer suzu needs explicit rules. The lexicon provides most of
them.

### 3.1 The lexicon is the sharing layer

Applications coexist because they all speak *rings*, not pixel commands.
Four apps don't fragment a 5×5 matrix into four bespoke protocols; each
emits `completion`, `alert`, `discovery` — and the companion implements
each signal once. Sharing is not an API feature; it is what a shared
vocabulary *is*.

### 3.2 Ground tenancy vs ring access

The face belongs to the machine's self-report — the primary producer.
Other applications never own the face; they **visit with rings**. A
backup app doesn't paint the LED; it requests a completion ring, which
overlays the ground for its duration, and the ground resumes
("ring, then return"). Where a producer *does* hold the ground (The
Run), tenancy is preemptible: **alert > run > ground**. This preserves
"give the server a face" no matter how many apps pile in: the machine
keeps its face; visitors speak and leave.

### 3.3 Attribution tokens

- Each producer owns a **hue** (generalize the firmware's `stone_hue`).
- Each producer owns a **copy prefix** for text devices ("backup:",
  "ci:").
- Text headlines rotate with attribution; worst-first, then
  oldest-voice-first.
- Where capacity allows, the matrix multiplexes spatially: one sprite
  per source, each wandering in its owner's hue.

At a glance, the room can tell *whose* story is being told.

### 3.4 Arbitration is modality-specific

- **Audio** — cricket's four channels are already a mixer: foreground
  interrupts, midground queues, background coexists. Debounce keys
  include the source, so one chatty app cannot hog the voice.
- **Visual ground** — single tenancy with the restore rule. A pixel
  cannot multiplex; it relies on tempo and restore.
- **Text headlines** — a rotation queue.
- **Pages (stories)** — queue with hold times; a page is an episode,
  not a fixture.

### 3.5 The budget is global, and it lives in the host

The enforcement point the PoC missed (it *declared*
`DeliveryPolicy::LatestEvery`/`Debounced` and never enforced them). The
host runs the console:

- per-producer **gain**: rate limits and debounce floors per source;
- a master **bus limit**: rings/minute across all sources;
- overflow behavior: coalesce into a **digest ring** ("three quiet
  things happened") rather than a stutter of flashes.

Without a central budget, sharing degrades into who-shouts-loudest, and
the calm budget dies at the second producer.

### 3.6 The command-path courtesy rule

Any local process can reach a companion (loopback web API, MCP, stdio).
The rule worth writing down: *programmatic clients speak rings and
ground; raw commands (`pixel`, `play`) are human- and agent-grade.*
This keeps the semantic layer authoritative while preserving the
visceral joy of `hey tell firefly fill 00ff00`.

## 4 · A Tuesday, shared

The server's self-report owns the ground (breathing, sage, Diorama page
ticking). 09:00 the backup app starts The Run — blue chase, progress
row, run beat on the pixel. 09:12 CI finishes a build — one green
completion bloom in CI's hue, water-can politely delayed by debounce,
headline notes "ci: build ok." 11:30 monitoring emits an alert —
everything else preempts, amber takes the matrix, the alarm sounds once,
and the whole house waits for the heal. Four applications, one bell,
zero negotiation — because each spoke rings, and the host mixed.

## 5 · What this asks of the contract

1. `source` attribution is already in the envelope — give it teeth:
   hue derivation, copy prefixing, debounce keys.
2. Tenancy and priority become explicit: a ground-claim request carries
   its class (`run`), rings carry their class (`alert`), and restore is
   implied.
3. The push endpoint exists for drive-by producers; the digest ring
   semantics exist for budget overflow.
4. Budget knobs (gain, bus limit) are host configuration with calm
   defaults.

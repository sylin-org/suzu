# Prior art & positioning

*Does suzu already exist? Where does it sit, what does it borrow, and where
is the defensible ground?*

Research date: 2026-08-28. Companion analysis:
[`poc-companion-surface.md`](poc-companion-surface.md).

---

## 1 · The short answer

The **idea** — software events expressed as calm physical signals in the
home — is thirty years old and well loved. The **articulation** — a small
open contract with self-owning companions, a hardware identity handshake,
and delight encoded in the protocol — does not exist as a named project.
The closest thing is the de facto DIY stack (Home Assistant + MQTT +
ESPHome/WLED/AWTRIX), which can imitate most of suzu's individual behaviors
but has a fundamentally different center of gravity. A search specifically
for "an open protocol/framework for software events → physical devices"
came up empty.

## 2 · The prior-art map (five lineages)

### 2.1 Calm-computing research — suzu's intellectual home

Mark Weiser's Xerox PARC vision, the Dangling String (network traffic as a
twitching string, 1995), the Ambient Orb (a frosted sphere shifting color
with stock/weather, 2002), AuraOrb (ambient light that reveals text on eye
contact), and academic work on ambient multisensory notification devices —
which is almost exactly suzu's thesis, studied as HCI research. Suzu is
*calm computing as an open protocol*; "calm tech certified" product
listings show the philosophy has gone mainstream. "The bell, not the alarm"
is no longer a fringe position.

### 2.2 Industrial — andon

Lean manufacturing's andon light stacks (red/amber/green over a machine)
are the direct ancestor. Suzu's heal-moment vocabulary is andon with
*narrative* — and that difference matters (§4.1).

### 2.3 DevOps — build lights

blink(1) has had a Jenkins plugin for a decade; Zalando wired USB traffic
lights to Jenkins; GitHub famously used Delcom lights. Proof that the
"software event → physical light" loop works and that people love it — but
every instance is a bespoke hack around one tool.

### 2.4 The DIY home ecosystem — the 800-pound gorilla

Home Assistant + MQTT + ESPHome/WLED is the de facto stack. The Ulanzi
TC001 flashed with AWTRIX 3 is a ~$40 commodity pixel-matrix companion that
HA drives over MQTT — functionally a firefly, mass-produced. Uptime Kuma
and Gatus do webhook alerts that people pipe into local chime scripts — a
poverty cricket. Critically, **HA's MQTT discovery is prior art for suzu's
command manifest**: devices declare themselves and capabilities appear
automatically. Take this ecosystem seriously; it can replicate every
*behavior* in the PoC today.

### 2.5 Agents — new and moving fast

Home Assistant's official MCP server and community projects (ha-mcp) give
agents tools to actuate the physical home, and "AI-powered local smart
home" is HA's explicit strategic direction. Suzu's MCP transport is *not*
novel as a concept — but its granularity is (§4.3).

## 3 · Where prior art beats suzu, honestly

A skeptic's version of suzu is: *"a few Home Assistant automations and an
AWTRIX clock."* That skeptic can replicate every PoC behavior today, for
less money, with a bigger community, without minting custom hardware. MQTT
discovery, presence detection, notification routing, pixel control, sounds
— all there, battle-tested for a decade. **If suzu's pitch is "events on
lights and sounds," it loses.**

## 4 · The defensible bets

### 4.1 Producers are software, not devices — and there is no protocol for that

Every prior-art system is *device-state* centric: motion detected, door
opened, CPU high. Suzu's events are *software lifecycle stories*: a backup
committed, a database healed, an offering replanted from a checkpoint. HA
has no vocabulary for "capture-committed" or "health-degraded-then-healed"
as first-class semantic events from arbitrary self-hosted software — you
would hand-write an automation per producer. The heal-moment vocabulary is
the actual invention: **a small, shared semantic vocabulary that any
producer can emit and any companion can express, without either knowing
the other.** Nobody has this. It is also the hardest thing to copy,
because it is taste, not code.

### 4.2 Companions own themselves; the contract is the center, not a hub

HA is hub-and-spoke: the hub is load-bearing, the devices are dumb. Suzu
inverts: each companion is a self-contained process with its own devices,
its own CLI (vesper), its own web API, its own identity — it runs with
zero suzu-brain present, and the host is just another peer. Delight is
encoded as *protocol defaults* (channels, debounce, ambient-baseline-
resume, "a companion that isn't running is resting") instead of
user-authored automation recipes. That is what makes "write a companion in
Python over a weekend" credible.

### 4.3 The MCP angle has a sharper edge than HA's

HA's MCP server exposes *the whole house* to an agent — powerful, and
exactly the blast radius you don't want an agent to have. Suzu's model —
each companion is itself a small MCP server whose tools derive from its
manifest, loopback-scoped, human-auditable via the same surface — gives
agents a **polite, scoped, gradated** way to touch the physical world: an
agent can make the bell ring; it cannot rewrite your automations. It also
doubles as an input path: an agent finishing a task is a producer emitting
`completion`.

### 4.4 The open-contract posture with teeth

"The contract is open; the SDK is optional" is only credible with a
conformance kit — the garden's ADR even says "fixture-tested." Nobody in
the DIY space publishes a testable device contract; they publish
integrations. A `suzu-fit` test suite (any language passes → your thing is
a companion) would be a first, and it is cheap to build from the PoC's
existing test-harness patterns.

## 5 · Ideation seeds

1. **Bridge, don't compete.** The pragmatic growth path is bridge
   companions: an MQTT bridge (HA events → suzu envelope), an
   ntfy/Uptime-Kuma/Gatus webhook bridge, a WLED or AWTRIX *firefly-class*
   companion. "The contract is open" then includes the entire existing
   hardware fleet, and every HA user becomes a potential suzu user without
   abandoning HA.
2. **Codify the delight budget as spec, not docs.** Debounce defaults,
   channel semantics, baseline-resume behavior, silence as a feature —
   write these into the contract as normative SHOULDs. It is the only
   moat that is uncopyable without adopting the philosophy.
3. **Producer onboarding = one JSON object.** The deliverable for a new
   producer should be "emit one envelope; document your kinds; done" —
   conformance fixtures make that real.
4. **Scope discipline as survival.** Suzu must resist becoming an
   automation engine, a dashboard, or a device-control hub — that is HA's
   turf and it would lose. The bell is the scope.
5. **The agent story deserves first-class treatment** — it is the axis
   where timing favors a small new entrant, because "agents in the home"
   is being decided now, and nobody is designing for agent-politeness.

## 6 · Strategic opportunity: the signal lexicon

> The service is as multimodal as the devices connected to it: a small
> ESP32 with a few pixels, a small mono screen, or a single-color pixel —
> all are visual outputs that can emit signals from a *known dictionary
> language*. "Emergency" can be a blinking red LED, a drawn exclamation
> mark on a mono OLED, or a full red danger icon on a 240×320 TFT.

This is the strongest untapped idea in the current design, and it deserves
first-class treatment in the contract.

### 6.1 The insight

An event carries **semantics** (`kind`: emergency, completion, heartbeat…).
A companion has **capabilities** (1 mono pixel; 5×5 RGB matrix; 128×64 mono
bitmap; 240×320 color TFT; speaker; someday haptics). Expression is a
*function*: `semantics × capability → concrete output`. Today CONTRACT.md
leaves `body` opaque and lets adapters consume producer payloads directly —
which makes every adapter know every producer (an N×M integration problem).
A shared **signal lexicon** turns that into N + M: producers emit semantic
signals; adapters implement *capability tiers*, not per-producer mappings.

The closest prior art is **MIDI**: a semantic music dictionary
(note-on/note-off, instrument-agnostic) where any instrument renders the
same notes according to what it can do. Suzu's lexicon is MIDI for
household awareness.

### 6.2 Capability tiers (visual sketch)

Expression is `semantics × capability`, where capability has **two axes**:
*spatial fidelity* (how much can be drawn) and *temporal depth* (how much
of the story can be held). Some devices display text, some mono images,
some full-screen content — all of them can tell stories, at different
depths.

| Tier | Device class | Example | Expression budget | Story depth |
|---|---|---|---|---|
| 0 | single mono LED | a lone pin | on / off / blink (tempo carries urgency) | the ending state only — the last ring's residue |
| 1 | few RGB pixels | 1–9 pixels | color tokens + pattern + tempo | ending state + tempo |
| 2 | small matrix | 5×5 RGB | glyphs, blooms, direction, the PoC's baseline-and-override | can sequence recent rings |
| 3 | mono bitmap / text | 128×64 OLED | drawn glyphs, exclamation marks, a text line | the **headline** — "redis healed 03:12" |
| 4 | color TFT | 240×320 | full iconography, layout, animation | the **page** — the whole arc: icon, headline, timeline |
| — | audio | cricket | channels + tunes (already tier-shaped: foreground/midground/background) | **peals** as tunes; ambient ground loops |
| — | future | haptics, servos | vibration patterns, motion | pattern tempo; held poses |

### 6.3 One signal, every device

`emergency` (red, pulsing, urgent) rendered down the tiers:

| Tier | Rendering |
|---|---|
| 0 | blink, fast, on |
| 1 | red pulse, urgent tempo |
| 2 | full-matrix red bloom / pulsing border |
| 3 | drawn `!` glyph, inverse-blink |
| 4 | red danger icon + label |
| audio | foreground alarm channel, high-priority tune |

The *semantic content survives the downgrade*. Tier-0 loses the icon but
keeps the severity (tempo), the valence (red/on), and the story (it began,
it ended). Graceful degradation is the rule, not an accident.

### 6.4 Design questions the lexicon raises (for ideation cycles)

- **Color tokens need mono equivalents.** Danger/warn/ok on RGB devices;
  urgency via *tempo* (blink rate, pulse speed) on mono ones. Consider
  color-blind-safe defaults from day one — tempo and pattern must carry
  meaning on their own.
- **Who owns the lexicon?** It should live in the contract core, versioned
  like everything else (`suzu/1`), open for new signals — the same
  governance as the event-kind vocabulary, which it complements: kinds are
  the *producer* side, lexicon signals the *consumer* side.
- **Naming.** Suzu is a bell; its expressions are **rings**. The dictionary
  is the ring vocabulary. Visual devices render rings as light, cricket as
  sound — one metaphor across modalities, per metaphor-as-architecture.
- **Conformance becomes renderable.** `suzu-fit` can include a test-card
  suite: render all tier-N lexicon signals, verify against reference
  captures (the live-harvest discipline applied to expression).
- **Escape hatches stay.** The envelope's opaque `body` and direct adapter
  commands (`pixel`, `fill`) remain for producer-specific flourish and
  human debugging; the lexicon is the portable core, not a ceiling.
- **Delight defaults live in the lexicon spec**: baseline-resume, debounce,
  channel mapping, silence-as-feature — normative SHOULDs, so taste
  propagates with adoption.
- **Arcs need envelope support.** Producers already emit stories in
  sequence (planted → rested, degraded → healed, capture started →
  committed); the envelope should make arc membership explicit — an
  optional `arc` id + `phase` (`begin | sustain | resolve`) — so adapters
  can group, render progress, and know when a story has ended.
- **Narrative capability is declarable.** Identity/manifest should carry
  what story depth the device holds (`ring | sequence | headline | page`)
  alongside hardware capability, so suzu routes the story at the depth the
  device can tell.

### 6.5 From signals to stories

Signals are atoms; the reason the heal-moment vocabulary exists is that
the household wants **stories, not status lines** (the garden's ADR-0006
says exactly this). Devices of every class can tell them — a single pixel
tells the present, a text line tells the headline, a full screen tells
the page. The lexicon therefore needs a small narrative grammar, and
campanology — the language of bell-ringing — supplies the names, on
liturgy:

- **ring** — an atomic semantic signal. The lexicon's words: emergency,
  completion, heartbeat, discovery, departure.
- **peal** — a structured sequence of rings forming an arc:
  begin → sustain → resolve. The alert then the heal-tune; the capture
  started then committed. Bell-ringers call structured sequences "changes";
  a peal is the story a set of rings tells together.
- **toll** — the ring class for departure and loss. The soft goodbye tune,
  the dot going dark. A bell vocabulary has always had a word for this.
- **ground** — the ambient baseline every visual companion keeps (presence,
  load tempo, the green storage firefly). Episodes interrupt the ground and
  resolve back into it. The PoC's baseline-and-override, generalized into
  grammar: ground → figure → return.
- **copy** — canonical one-line narration per ring ("captured," "healed,"
  "gone quiet") owned by the lexicon, not the producer, so a text device
  speaks producer-agnostic language. Per-locale variants belong to the
  lexicon too — the contract is the place where the house's language lives.

Concrete contract consequences:

1. The envelope gains optional `arc` + `phase` fields; the `subject` is the
   natural arc key ("the story of offering redis").
2. Identity/manifest gains a `lexicon` section: modality, tier, and story
   depth held (`holds: ring | sequence | headline | page`).
3. `suzu-fit` grows story cards alongside test-cards: feed a scripted peal,
   capture what each tier renders, diff against reference.

Scope guard: **a page is not a dashboard.** A page is a ring or peal
rendered at full fidelity — an episode with a beginning and an end — not a
live data surface. The contract specifies *what to tell*; the firmware
decides *how to draw*. Firefly's 30 fps sprite engine stays in the
companion.

With the lexicon, the pitch sharpens into something defensible:
*producers speak stories, companions speak rings, and the lexicon is the
shared language between them — rendered at whatever depth the device can
hold, from a single pixel keeping the present to a page keeping the whole
arc.*

## Sources

- [Ambient Orb (NBC News, 2004)](https://www.nbcnews.com/id/wbna4758931)
- [Designing Ambient Multisensory Notification Devices (ACM, 2020)](https://dl.acm.org/doi/10.1145/3428361.3428400)
- [AuraOrb: Social Notification Appliance (CHI 2006)](https://www.interruptions.net/literature/Altosaar-CHI06-p381-altosaar.pdf)
- [Exercises in Calm Technology](https://calmtech.com/exercises)
- [Calm Technology in the Era of Push Notifications (Delve)](https://www.delve.com/insights/calm-technology-in-the-era-of-push-notifications)
- [Calm Tech Certified product list (2025)](https://www.calmtech.institute/post/the-complete-calm-tech-certified-product-list-for-2025)
- [AWTRIX 3 on the Ulanzi TC001 through Home Assistant](https://mattzaskeonline.info/blog/2025-07/configuringusing-awtrix-3-ulanzi-tc001-through-home-assistant)
- [Ulanzi TC001: Custom LED Messages with Home Assistant](https://hometechhacker.com/ulanzi-tc001-custom-led-messages-with-home-assistant/)
- [Signalling Jenkins build status with a mini USB traffic light (Zalando, 2017)](https://engineering.zalando.com/posts/2017/06/signalling-your-jenkins-build-status-with-a-mini-usb-traffic-light.html)
- [blink(1) Notifier for Jenkins](https://plugins.jenkins.io/blink1/)
- [LED Tree Jenkins Build Monitor (Hackaday)](https://hackaday.io/project/19038-led-tree-jenkins-build-monitor)
- [Home Assistant MCP Server integration](https://www.home-assistant.io/integrations/mcp_server/)
- [ha-mcp: Home Assistant MCP server (community)](https://github.com/homeassistant-ai/ha-mcp)
- [Building the AI-powered local smart home (Home Assistant blog, 2025)](https://www.home-assistant.io/blog/2025/09/11/ai-in-home-assistant/)
- [Uptime Kuma: notification sound discussion](https://github.com/louislam/uptime-kuma/issues/4737)
- [Gatus — self-hosted monitoring with notification channels](https://github.com/TwiN/gatus)
- [BusyLight for Humans (multi-vendor USB light library)](https://github.com/JnyJny/busylight)
- [awesome-mqtt — the MQTT ecosystem index](https://github.com/awesome-mqtt/awesome-mqtt)
- [teams-for-linux MQTT busy-light integration](https://github.com/IsmaelMartinez/teams-for-linux/blob/main/docs-site/docs/mqtt-integration.md)

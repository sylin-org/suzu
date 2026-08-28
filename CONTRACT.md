# The Suzu contract

The protocol between producers and companions. Three types, transport-agnostic,
versioned. If these shapes change, the version bumps.

## 1 · Event envelope

What a producer emits. JSON, one shape for all event kinds.

```json
{
  "proto": "suzu/1",
  "kind": "transition",
  "source": "zen-garden",
  "ts": "2026-08-28T00:30:00Z",
  "subject": "mongodb::default",
  "body": {
    "from": "degraded",
    "to": "running",
    "stone": "stone-tranquil-pass"
  }
}
```

| Field | Required | Meaning |
|---|---|---|
| `proto` | yes | schema version, currently `"suzu/1"` |
| `kind` | yes | the event's semantic type (see vocabulary below) |
| `source` | yes | the producer's identity (e.g. `"zen-garden"`) |
| `ts` | yes | when the event occurred |
| `subject` | no | what the event is about (an offering name, a stone name, a bucket) |
| `body` | no | producer-specific payload — opaque to Suzu, consumed by adapters |

### Event kind vocabulary

Kinds are open — producers may define their own. The seeds:

| Kind | Meaning | Example body |
|---|---|---|
| `heartbeat` | periodic liveness | `{"uptime_s": 5654500}` |
| `transition` | a state changed | `{"from": "degraded", "to": "running"}` |
| `alert` | needs attention | `{"severity": "warn", "message": "..."}` |
| `completion` | a long operation finished | `{"operation": "capture", "hash": "..."}` |
| `discovery` | something new appeared | `{"device": "firefly-001"}` |
| `departure` | something went away | `{"reason": "expired"}` |

### The heal-moment vocabulary (Zen Garden's first instantiation)

| Zen Garden event | Suzu kind | Delight |
|---|---|---|
| `StoneSeen` | `heartbeat` | firefly: the stone's dot pulses |
| `StoneGoodbye` | `departure` | cricket: a soft goodbye tune |
| `OfferingPlanted` | `discovery` | firefly: a new dot appears |
| `OfferingRested` | `transition` | firefly: the dot dims |
| `CaptureCommitted` | `completion` | cricket: the water-can tune; firefly: brief bloom |
| `HealthDegraded` | `alert` | cricket: alarm; firefly: the dot turns amber |
| `HealthHealed` | `transition` | cricket: the heal tune; firefly: the dot returns green |
| `Replanted` | `discovery` | cricket: the replant fanfare; firefly: full-matrix bloom |

## 2 · Command manifest

What a companion declares it can do. Published at startup, discoverable
via any transport.

```json
{
  "proto": "suzu/1",
  "companion": "firefly",
  "version": "1.0.0",
  "transports": ["sse", "web", "mcp", "cli", "stdio"],
  "commands": [
    { "name": "status", "description": "Current device state" },
    { "name": "pixel", "description": "Set one pixel",
      "args": [
        { "name": "x", "type": "u32" },
        { "name": "y", "type": "u32" },
        { "name": "color", "type": "string" }
      ] },
    { "name": "fill", "description": "Fill all pixels",
      "args": [{ "name": "color", "type": "string" }] },
    { "name": "brightness", "description": "Set brightness (0-255)",
      "args": [{ "name": "level", "type": "u8" }] }
  ]
}
```

Every transport derives from this declaration: the CLI generates subcommands,
the MCP server generates tools, the web API generates endpoints. One
declaration, many mouths.

## 3 · Identity handshake

How a hardware device proves it's a Suzu companion. Write the byte `I` to
the serial port; expect a JSON identity response within 4 seconds.

```json
{
  "proto": "suzu/1",
  "companion": "firefly",
  "family": "rp2040-matrix",
  "device_id": "019a...",
  "firmware": "1.0.0",
  "pixels": 25
}
```

The 4-second deadline is a compile-time assert. A device that cannot answer
in 4 seconds is not a Suzu companion — it is a serial port that happens to
be attached.

## 4 · Transport notes

| Transport | Direction | Notes |
|---|---|---|
| SSE | producer → Suzu | HTTP Server-Sent Events; backoff 1→32s on disconnect; 50ms coalescing |
| CLI | bidirectional | `vesper` binary; the companion's own rake |
| Web API | bidirectional | REST on the companion's loopback port; port assigned by the host or self-selected |
| MCP | companion serves | the companion IS an MCP server; tools derive from the command manifest |
| stdio | bidirectional | line-delimited JSON on stdin/stdout; for embedding and testing |

## 5 · Ownership

Each companion process owns its own devices and its own lifecycle. The host
(however it runs companions) sends events, assigns ports, and tracks
liveness — it does not reach into the companion's internals. A companion
that isn't running is a companion that's resting.

# The face contract

*What every suzu face does, regardless of dialect. The OLED writes in
glowing text and icons; the 5×5 tells in light. This contract is what
makes them the same animal.*

---

## 0 · The one law of voices

The host has the firepower; the face has the truth. The resident
reduces every request into each device's own dialect before the wire —
**the serial hop carries only what that device can render**. A face
never parses intent. A face never stores strategy. It receives its
language, renders it, and answers the handshake.

## 1 · The phases

```
[splash?] → idle → (wake) → work ⇄ ring overlay → ground → idle …
```

- **splash** — optional, ≤2 s, before any communication. Short and
  sweet, or absent entirely (an instant boot is a legitimate "no
  splash"). The resident's probes already tolerate the boot window.
- **idle** — the root state. No data = idle; silence returns here; boot
  opens here. Each face defines its own garden (the OLED: firefly
  flights and the glowing label; the 5×5: fireflies over dark water).
  Idle is *honest absence* — no data is dressed as data.
- **wake** — the optional idle→work transition (the OLED's accelerating
  fireflies; the 5×5's gather-and-pop). A moment in its own right.
- **work** — the ground: the face renders the house's truth in its
  declared slots and dialect, at its own rendering policy.
- **ring overlay** — a moment borrows the face for its natural duration
  (or latches, for alerts), then the ground resumes. Nothing is
  cleared: states own the truth.

Silence decays home: 10 s without frames returns any non-idle face to
idle — except a latched alert, which never idles away. That is the
honest failure mode: danger persisting unconfirmed stays visible.

## 2 · Reserved vocabulary (every device, every dialect)

- **`label`** — the house's name for this firefly. Persistent
  (survives reboots and reinstalls), restored at boot, written by one
  path, stamped by one drawer, **never reverted** by shutdown, silence,
  or any other frame. Faces that cannot show text keep it anyway: it is
  identity, not decoration.
- **`name`** — reserved as the contract's term for that label.
- **the nine rings** — alert, allclear, completion, discovery, begin,
  departure, tended, transition, heartbeat — with qualifier
  degradation: `alert.disk` matches by verb; unknown qualifiers never
  disconnect the objective.
- **urgency 0–5** — the vitality scale, rendered as the device's tempo
  (the gloss: 5–4 breath, 3 pulse, 2 blink, 1 strobe, 0 dark).

## 3 · The say surface

External programs and the Keeper speak through the resident:

```
suzu say <ring>[.<qualifier>] [text ...]     # a moment, on faces
suzu say allclear[.qualifier]                # heals a latched alert
suzu pause | suzu resume                     # stop/restart the stream
suzu screenshot | suzu record <s> <fps>      # the trail camera
```

The moments domain applies its budget (bursts coalesce), resolves
attribution, and reduces per device. An informational *level* ("disk
at 50%") is a data atom and belongs to ground; a *warning* ("disk at
95%") is a ring (`alert.disk`) that latches. The objective picks the
frame.

## 4 · Rendering per dialect

| Dialect | Owns | Sentence |
|---|---|---|
| **light** (5×5 matrix) | hue = valence, intensity = urgency, tempo = the gloss, pattern = the story | raindrops and atom fireflies |
| **text+icons** (OLED v2) | glowing spine label, icons, band blink | the portrait composition |
| **terminal** | words, timestamps, structure | the full line |
| **ancestor** (pre-suzu) | its own installed voice | `WIPE-IN`, wipes, its dashboard — visibly non-lossy |

A device that cannot render a moment's *icon* still renders its
*objective* (severity, tempo, words where possible) — the icon is
complementary; the communication is the contract.

## 5 · The laws (inherited, restated)

1. **Report history, never biography** — a tool states what happened;
   it never concludes what the hardware is.
2. **The tool that bricks is the tool that un-bricks** — every write
   path has its rollback in the same hand; backup precedes every write.
3. **No data = idle** — absence is honest; nothing is dressed as data.
4. **`label` never reverts** — one writer, persisted, restored.
5. **Alerts latch** — danger persisting unconfirmed stays visible until
   `allclear` or `X`.
6. **The host reduces** — the wire carries only what the device renders.
7. **Art is data** — the tool installs sprites, fonts, bytecode; it
   never interprets them.
8. **Verify, then say so** — no success claims ahead of the handshake.

## 6 · The shot law (the trail camera)

A face answers

```
J,{"shot":1}
```

with `OK,<base64(raw framebuffer)>*hh` inside its normal poll loop —
no reboot, no mode change, the animation keeps dancing while the host
reads it. The `*hh` xor checksum covers the whole outgoing line
(everything before the `*`). The J reply **is** the liveness proof:
no identity probe gates a shot, and a port that doesn't answer gets
one honest line.

Devices ship RAW memory; the host interprets (ADR-0001). Only the
frame bytes differ per device, and the class manifest's `frame:`
section is the only per-device knowledge — size, format, depth,
order, palette — plus a `render:` hint (mount rotation, upscale) so
the host shows what the eye sees. One generic decoder; a new face
ships with a manifest entry, never a host code change.

- **`suzu screenshot [port]`** — one PNG per connected face
  (`shot-<port>.png`), decoded per *that device's* manifest; suzu
  coordinates, the caller never walks ports or formats.
- **`suzu record <secs> <fps> [port]`** — the same in-band capture
  looped at a wire-respecting rate (clamps: 1–60 s, 1–5 fps) against
  the first answering face, assembled as an animated GIF
  (`record-<port>.gif`). Each shot costs the face one ack-sized write
  (~120 ms) — the wire, not the encoder, is the tax.

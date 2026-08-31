# hardware/ — the fleet's memory

Folder per class, one concern per file, grown by adoption:

```
classes/<class-id>/
├── signature.yaml     identification bits — what `suzu` parses at boot
├── manifest.yaml      coverage target, sets, constraints, boards, migration
├── procedure.yaml     flash checklists (closed step vocabulary)
└── evidence/          dated observations, one file per bench session
descriptors/           the suzu/1 identity templates
```

Three layers, one owner each:

- **class** (the signature) → coverage, procedures, sets
- **incarnation** (the board) → the `boards` list in `manifest.yaml`
- **individual** (device_id) → `evidence/`; survives upgrades

Evidence levels: `observed` (bench-grounded, dated) beats `suspected`
(vendor listing). The probe (`I`, 4 s) is the authority; VID/PID is a
bridge hint. A new physical board sharing a signature joins that
class's `boards`; a genuinely new signature gets a new class folder —
that's the whole contribution flow.

## Tool

The `suzu` binary loads every `classes/*/signature.yaml` at boot and
answers from them:

- `suzu` — watch: identify on hotplug, service devices
- `suzu scan` — one-shot identification
- `suzu detective` — full fact dump per USB device (USB descriptors,
  probe transcript, catalog matches, a draft `signature.yaml`) — attach
  its output to a class proposal

The built-in seed hints are fallback only; these folders are the source
of truth.

## Language

firefly (visual) and cricket (audio) are suzu's own monikers. Ancestor
firmware is called *ancestor* — a device state, never an identity.
History stays in the harvest docs, never in these files.

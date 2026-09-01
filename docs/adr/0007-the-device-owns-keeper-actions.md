# ADR-0007: The device owns the keeper's verbs

**Status:** accepted  
**Date:** 2026-08-31

## Context

Workbench originally inferred its buttons from lifecycle strings while the
Resident independently validated pause, resume, identify, and maintenance
commands. Adding a terminal surface would have created a third copy. The copies
had already drifted: a paused card offered Identify even though the live session
correctly refused it.

The old `Device` also mixed two different things. Identity and recognized facts
sat beside a serial session's mailbox, close flag, stream gate, and frame
capability. That made it difficult to say whether a method represented a domain
decision or transport bookkeeping.

## Decision

The minded `Device` is the aggregate that owns the keeper action vocabulary:

- `device.pause()` and `device.resume()` apply lifecycle law through the roster;
- `device.identify()` requires a live individual;
- `device.install(faceplate)`, `device.update(faceplate)`, and
  `device.factory_reset()` validate state and declared dresses, then return a
  typed maintenance order;
- `device.available_actions()` publishes the legal verbs on every device row.

The `Devices` actor is the application service. It enacts typed orders by
moving session gates, publishing facts, and starting maintenance workers. It
does not decide what a lifecycle permits. Serial mailboxes, close flags, and
streaming flags live in `SessionHandle`, outside the entity.

Every presentation reads `DeviceRow.actions`. Workbench chooses button labels;
`suzu list` chooses terminal labels. Neither infers permission from `NEW`,
`LIVE`, or `PAUSED`. Both call device-member doors that mirror the aggregate:

```text
POST /api/device/P/pause
POST /api/device/P/resume
POST /api/device/P/identify
POST /api/device/P/install         {"faceplate":"slate"}
POST /api/device/P/update          {"faceplate":"slate-left"}
POST /api/device/P/factory-reset
```

The older maintenance and identify doors remain thin compatibility adapters;
they immediately translate into the same `DeviceAction` command.

## Consequences

- A new surface gets the same capabilities by consuming the read model and
  action doors; it cannot silently invent a legal transition.
- A new device verb begins as one enum variant and one aggregate method. The
  actor receives a typed order, and presentations receive the new name in the
  same snapshot.
- Faceplate vocabulary is checked before serial ownership changes, so an
  unknown dress is a domain refusal rather than a half-started saga.
- Long-running work remains asynchronous. The action reply confirms ownership;
  roster and journal facts carry progress and the admission verdict.
- The aggregate depends on the roster's lifecycle model and catalog vocabulary,
  but not on HTTP, Tauri, terminal I/O, Tokio tasks, or serial-port mechanics.

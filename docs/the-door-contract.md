# The door contract

*How every command door in the Resident's loopback API speaks.*
Adopted 2026-08-30, born of the `watch_media` exchange (ADR-0004, the
watched lane). Kin to [`the-face-contract.md`](the-face-contract.md):
the same honesty, on the host's side of the wire.

---

## 1 · The ask is terse

One subject, one state, no envelope around it:

```
POST /api/ui    {"watch_media":"on"}
POST /api/control {"verb":"pause"}
```

- The **subject is the key**, worded as the thing being toggled or
  asked for — never an `action`/`type` wrapper, never a nested object
  where a word will do.
- The **value is the state requested**, not an imperative: `"on"`,
  not `"enable"`. The ask reads the same in a log, a curl, or a doc.
- Unknown values are refused **by name**: the refusal states the
  door's whole vocabulary, so the next try needs no documentation.

## 2 · The answer is always three things

Every command door — success, no-op, refusal, or timeout — answers:

```json
{"confirmed": true,  "watch_media": "on",  "message": "Streaming captures on 2 devices"}
{"confirmed": false, "watch_media": "on",  "message": "Streaming already enabled (2 devices)"}
```

1. **`confirmed`** — did the house *change* anything. A no-op is not
   an error: it is `confirmed:false` plus the "already" truth. The
   client that only needs to know "is it so now" reads the echo;
   the client that needs to know "did my click matter" reads
   `confirmed`.
2. **The echo** — the subject keyed as asked, valued as asked (or as
   it landed, when the door says so). A client correlates its own
   request without keeping a ledger.
3. **`message`** — one human sentence of what is *now true*, with
   concrete counts, paths, and ports where they exist. It is written
   for the Log as much as for a toast: the same string may travel
   both. It never says "OK" and never says "success" — it says what
   happened, in the house's own voice.

The echo carries whatever the door was given: a word (`"on"`), a
parameter object (`{"secs":4,"fps":3}`), or a result that *is* the
answer (`"saved"` → the path).

## 3 · The status code is the transport's truth

| code | meaning |
|------|---------|
| 200  | the house answered — changed or no-op alike |
| 400  | the ask is not in this door's vocabulary |
| 404  | the ask names nothing the house knows |
| 409  | the house refuses — the state's own law says no |
| 504  | the house did not answer within its bound |

The body carries the envelope in every one of these rows. The code is
for transports and curl scripts; the body is for humans and logs.

## 4 · The doors

| door | ask | subject echo |
|------|-----|--------------|
| `/api/ui` | `{"watch_media":"on"\|"off"}` | `"watch_media"` |
| `/api/control` | `{"verb":"pause"\|"resume"}` | `"verb"` |
| `/api/device/P/pause` · `/resume` | — | `"pause"`: `"off"` · `"resume"`: `"on"` |
| `/api/device/P/identify` | — | `"identify"`: the port |
| `/api/device/P/install` | `{"faceplate":…}` (optional) | `"install"`: the chosen faceplate |
| `/api/device/P/update` | `{"faceplate":…}` (optional) | `"update"`: the chosen faceplate |
| `/api/device/P/factory-reset` | — | `"factory_reset"`: `true` |
| `/api/admission/P` | — | `"admission"`: `"retry"` |
| `/api/record/P` | `{"secs":…,"fps":…}` | `"record"`: `{"secs","fps"}` |
| `/api/capture/P/save` | — | `"saved"`: the path |
| `/api/say` | `{"kind","label","urgency"}` | `"say"`: the kind |
| `/api/shutdown` | — | `"stopping"`: `true` |

Read doors (`/api/events`, `/api/log`, `/api/destinations`,
`/api/device-image`, `/api/faceplates`, `/api/shot`) are not command doors: the stream is
the whole truth and the reads are curl-only conveniences or assets.
They keep their own shapes.

The older `/api/maintenance/P` and `GET /api/device/identify/P` shapes
remain compatibility adapters. They
translate immediately into the device aggregate's same typed methods;
new clients use the device-member doors above.

## 5 · The law behind it

A door that answers "ok" has said nothing. A door that answers in a
shape the client must interpret per-door has made the client a parser
of dialects. The envelope is one dialect: *was anything changed, what
was asked, what is true now* — in that order, on every door, so a
keeper watching the log reads sentences, and a script written against
one door works against all of them.

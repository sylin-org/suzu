# Delight & ease-of-use assessment

*Is suzu delightful and easy? For whom, where are the cliffs, and how do
we test it?*

Assessment date: 2026-08-28. Under [`the-model.md`](the-model.md) and
[`implementation-plan.md`](implementation-plan.md).

---

## 1 · Verdict

**The design is delight-dense and ease-correct at its core — delight is
concentrated in exactly the right place (the ground), and every persona's
first success is achievable quickly. The remaining delight risk is
entirely in execution defaults**, not in further design. The system will
feel exactly as calm, present, and legible as the shipped faces, budgets,
and install path actually are.

## 2 · Where the delight is structural

- **The ground is the product.** Rings are seconds per day; breath,
  hearth, and the quiet report are the other 99%. The PoC firmware
  already proves the ground can be lovely; suzu inherits it, and faces
  scale with makers (timbre), not with the core team.
- **The emotional beats are reachable with the designed vocabulary:**
  adoption (mint → name → hue → first breath), the goodnight (tend →
  sparkle → chime), the morning report (news that waits on the shelf),
  the absence (the stopped clock — protected by rest ≠ comms-loss ≠
  dead), the heal (degraded blink → drive arrives → bloom → breath).
- **A quiet one:** the datum's ready-made text (`"50GB out of 2TB
  used"`) means display makers ship thoughtful wording they never
  wrote. Producers carry the prose; companions carry the poetry of
  layout.

## 3 · Ease by persona

| Persona | Ease | Notes |
|---|---|---|
| Owner | high | zero-config core loop: plug → HELLO → adopted → breathing |
| Household | perfect | a breathing thing on a shelf; nothing to learn, by omission |
| Maker | high at core | five opcodes, `strstr` parser, `cat` debugging, virtual-device `suzu fit`; but knowledge spans eight docs — quickstart needed |
| Producer dev | high | first ring is one `curl`; self-report needs no code; sets are optional depth |
| Agent | n/a | MCP-scoped and polite; the granularity makes it the agent's easy problem |

## 4 · The three cliffs

1. **Firmware flashing is the new driver hell.** The PoC's provisioning
   was PowerShell + mpremote — the *first* thing every new owner must
   do. Treat as core scope: browser-based flashing, pre-flashed
   partners, and the **terminal face as the zero-flash default**.
   (Developed further in
   [`hardware-catalog-and-adoption.md`](hardware-catalog-and-adoption.md).)
2. **No maker quickstart.** The weekend claim needs its artifact: a
   ~50-line "hello face" skeleton (answers `I`, breathes, renders `R`
   by tempo) plus `suzu fit` passing on a virtual device.
3. **Jargon leakage.** Rings, tolls, peals, grounds, atoms, folds —
   delight for contributors, friction for users. Rule: *internal
   poetry, external plainness.* `suzu adopt firefly`, never
   "register companion with coverage class". The PoC got this right
   (`hey tell cricket volume 50`).

## 5 · The six tests (acceptance criteria)

| # | Test | Gate |
|---|---|---|
| 1 | **60-second test** — install suzud, run vesper: something breathes in a terminal, zero hardware | M2 |
| 2 | **10-minute test** — unowned device → adopted → breathing on real hardware, including flashing | M3 |
| 3 | **Glance test** — one glance answers "is everything okay?" from across the room | M4 |
| 4 | **Absence test** — a server dies; a human notices in seconds without alarms; no false alarm on routine reboots | M4 + fit |
| 5 | **Guest test** — a non-technical visitor smiles or asks, unprompted | M4/M5 hardware gates |
| 6 | **Weekend test** — a stranger ships a working companion in a weekend from the quickstart alone | quickstart + virtual device |

## 6 · The first ten minutes (journey spec)

```
0:00  install suzud — one line; service registration optional
0:30  `suzu` with nothing attached: "plug a device, or run `suzu face`"
1:00  `suzu face` — the terminal breathes            (delight beat #1)
3:00  plug a device — `suzu adopt` identifies it
5:00  name it; hue derives; first breath             (delight beat #2)
8:00  first real ring — self-report or one webhook curl
10:00 walk away; glance back — the glance test begins
```

Exit criteria: tests 1, 2, 3 pass on this journey alone.

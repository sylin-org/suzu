# The release process

One tag builds everything; one click publishes everywhere. The design
rule: every step a machine can do, a machine does — the human acts at
exactly two points, both of them judgment calls.

```
git tag -a vX.Y.Z -m "…"            # 1. the human decides it ships
git push git@github.com:sylin-org/suzu.git vX.Y.Z
        │
        ▼  release.yml (on tag push)
windows · linux-gnu · linux-musl archives → SHA256SUMS → latest.json
        │        → draft-worthy GitHub Release, notes from CHANGELOG.md
        ▼  publish.yml (on release published)      # 2. the human clicks
brew formula → sylin-org/homebrew-tap
npm shim    → @sylin-org/suzu
winget PR   → microsoft/winget-pkgs
website     → versionFallback bump in sylin-org/website
```

The two human gates are different in kind. The tag says *this tree is
the release*; publishing the release says *send it to the world*. A
built-but-unpublished release can be deleted and nothing downstream
ever heard of it.

## Before tagging

- `main` is green (CI runs the suite and clippy on Windows and Linux).
- `CHANGELOG.md` carries an entry for the new version — its body
  becomes the release notes.
- The tag message is one human line; the changelog does the talking.

## What each channel needs

| channel | artifact | CI job | secret |
|---|---|---|---|
| GitHub Releases | archives + `SHA256SUMS` + `latest.json` | `release.yml` | none |
| Homebrew | `Formula/suzu.rb` rendered from the checksums | `publish.yml → tap` | `TAP_TOKEN` |
| npm | `@sylin-org/suzu` shim (downloads at install) | `publish.yml → npm` | `NPM_TOKEN` |
| winget | manifest PR to `microsoft/winget-pkgs` | `publish.yml → winget` | `WINGET_TOKEN` |
| Website | `versionFallback` bump (runtime overlay already carries the live version) | `publish.yml → website` | `WEBSITE_TOKEN` |

Every publish job **skips with a notice** when its secret is absent —
missing configuration never fails a release, it just leaves that
channel dark until the secret appears.

## The secrets (one-time setup)

- `TAP_TOKEN`, `WEBSITE_TOKEN` — fine-grained GitHub PATs, Contents
  read/write, scoped to `sylin-org/homebrew-tap` and `sylin-org/website`
  respectively. (One PAT may seed both; separate secrets allow
  independent rotation.) Fine-grained PATs for org repos need the org's
  settings to permit them.
- `NPM_TOKEN` — an npm token with publish rights on `@sylin-org`
  (granular, packages read/write, is the durable choice). Note npm's
  announced horizon: tokens that bypass 2FA stop being able to publish
  directly in January 2027 — mint granular, not legacy-automation.
- `WINGET_TOKEN` — a classic GitHub PAT with `public_repo`, because
  opening the PR to `microsoft/winget-pkgs` acts outside our org.

Set with: `gh secret set NAME --repo sylin-org/suzu`. Local copies of
the values live only in the maintainer's gitignored `.ignore/` store.

## Not automated (on purpose, for now)

- **crates.io** — blocked until the resource trees move inside the
  crate root (the v0.2 layout fix); then a `CARGO_REGISTRY_TOKEN`
  job joins this file.
- **Testbed rollout** — hosted runners cannot reach the bench LAN.
  The drill stays manual: `git pull && cargo build --release -p suzu
  && sudo suzu install`, or unpack the release archive directly.
- **The tap's ghostlight formula** — untouched by our automation; the
  bot writes `Formula/suzu.rb` only.

## Verifying a release, by hand

```
sha256sum -c SHA256SUMS
./suzu version                # says the tag
./suzu scan                   # catalog loads from the embedded resources
brew install sylin-org/tap/suzu && suzu version
npx @sylin-org/suzu version
```

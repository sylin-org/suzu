# Test platforms — service testbeds

The Resident is developed on a Windows bench, but it ships to Linux hosts.
Three machines on the bench LAN serve as disposable test platforms: they
exercise the build, the packaging, and the service itself across
different Linux families before anything lands on a long-lived host.
Their lifecycle follows ADR-0008 — deployment is scripted and repeatable,
and nothing on them is precious.

| host | OS | init | role |
|---|---|---|---|
| `test-01` | CachyOS (Arch family) | systemd | glibc + systemd reference; primary build host |
| `test-02` | Bluefin 44 (Fedora atomic) | systemd | immutable-distro packaging: `/usr` is read-only, `/usr/local` and `/etc` are not |
| `test-03` | Alpine 3.24 (musl) | OpenRC | musl build; the no-systemd adaptation |

The deployment template is **stone-halcyon-savanna** (Debian 13), the
long-lived host the Resident actually serves from. All four follow the
same recipe; the test platforms differ only where their init or libc
forces it.

## The deployment pattern

Per host, as the service user (historically `stone` on the template,
`test` on the testbeds):

1. Rust toolchain in `$HOME` (rustup; no system packages needed).
2. Repo cloned to `~/repos/github/sylin-org/suzu` from
   `https://github.com/sylin-org/suzu.git`.
3. GitHub credentials in `~/.git-credentials` with
   `git config --global credential.helper store`; Codex CLI
   credentials in `~/.codex/`.
4. `cargo build --release -p suzu` (the `-p suzu` matters: the workbench
   crate needs GTK development headers that service hosts rightly lack),
   then `sudo suzu install` — the Resident deploys itself: binary to
   `/usr/local/bin/suzu`, resources to `/usr/local/share/suzu`, udev rule
   `60-suzu.rules`, the `suzu-hw` group, and the service definition for
   whichever init the host runs (systemd, or OpenRC on musl hosts). The
   binary carries its own manifests and payloads, so the install needs
   no checkout at all — a relayed binary works from anywhere.
   `suzu install --verify` checks an installed host;
   `scripts/install-linux.sh` remains the ancestor reference for
   installing from a checkout without running the new binary first.

Credentials are disposable and live only in the maintainers' local
`.ignore/` store (gitignored); they are never committed.

## Per-host deviations (as proven on the bench, 2026-09-01)

- **test-03 (Alpine)** has no systemd. The binary, resources, and udev
  rule install the same way; the service runs under an OpenRC script at
  `/etc/init.d/suzu` (`rc-update add suzu default`). The lesson its
  bench taught: **log files redirected by the init script must be
  writable by the service user** — files created by an earlier root-run
  silently killed every dropped-privilege child with exit code 1. If an
  OpenRC service file graduates from testbed to standard, it moves to
  `crates/suzu/deploy/` with its own promotion note.
- **test-02 (Bluefin, Fedora atomic)** cannot build natively yet:
  `libudev-sys` needs `libudev.h`, and `rpm-ostree install systemd-devel`
  refuses to apply because `/usr/local` is a symlink ("changed
  directories are not supported yet"). Since the binary carries its own
  manifests and payloads, the testbed installs **checkout-free**: the
  binary built on test-01 is relayed and `sudo ~/suzu-binary install`
  runs from the home directory — resources land under
  `/var/usrlocal/share/suzu`, the atomic image's `/usr/local`, and the
  cross-distro glibc portability check passes. Promoting test-02 to a
  native build needs an unblocked layering path, kept as an open item.

## What the testbeds are for

- **Build portability**: glibc vs musl, Arch vs Fedora packaging
  assumptions in `scripts/install-linux.sh`.
- **Service behavior**: `suzu serve` under a real init system, restart
  policy, state directory (`/var/lib/suzu/<user>`), and resource
  resolution from `/usr/local/share/suzu`.
- **Upgrade drills**: `git pull && cargo build --release` +
  `scripts/install-linux.sh` as the update path, before it is trusted
  on the serving host.

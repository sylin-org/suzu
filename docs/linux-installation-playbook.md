# Linux installation playbook

This is the runbook for a person or an agent installing Suzu on a Linux host.
It produces the same layout on Debian, Arch, Fedora, and other systemd-based
distributions while keeping distro packaging separate from runtime setup.

The result is one system service instance, `suzu@KEEPER.service`, running as
the existing unprivileged keeper account. It owns the serial stream, starts at
boot, writes only beneath `/var/lib/suzu/KEEPER`, and reaches supported devices
through the narrowly scoped `suzu-hw` group and udev rules.

## The separation to preserve

1. **Delivery** puts the binary, catalog, firmware, unit, and udev policy on
   disk. A distro package should do this; `scripts/install-linux.sh` is the
   source-checkout fallback.
2. **Machine setup** creates `suzu-hw`, reloads systemd/udev, and enables the
   chosen `suzu@KEEPER` instance. It never runs the desktop UI as root.
3. **Hardware adoption** is a later, explicit `suzu prepare` or Workbench
   action. Installing the host service must not flash or rewrite a companion.

CLI-only and GUI-directed installs must converge on this same service and file
layout. A GUI may present the decisions and invoke a privileged package/setup
helper, but it must not grow a second installation implementation.

## 1. Preflight

Run read-only discovery first:

```sh
cat /etc/os-release
uname -m
ps -p 1 -o comm=
id
systemctl --version
```

Stop if PID 1 is not systemd; this repository does not yet ship OpenRC, runit,
or launchd service definitions. Record the keeper with `id -un` before using
`sudo`. The account must be a normal, existing local account, not `root`.

Check for a conflicting Resident or an older proof of concept:

```sh
systemctl list-unit-files | grep -Ei 'suzu|koan|zen.garden' || true
ss -lntup | grep -E ':(7898|7899)[[:space:]]' || true
```

Do not delete an unfamiliar service merely because it matches. Inspect its
unit and ask the owner when provenance is unclear. Exactly one process may own
Suzu's loopback door and each serial port.

## 2. Build dependencies

Install the Rust toolchain using the distribution's policy, then install the C
compiler, `pkg-config`, and libudev headers needed by the `serialport` crate.
Typical package sets are:

```sh
# Debian / Ubuntu
sudo apt install build-essential pkg-config libudev-dev

# Arch Linux
sudo pacman -S --needed base-devel pkgconf systemd

# Fedora
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config systemd-devel
```

Package names change; an agent should query the active distro if one of these
names is unavailable rather than substituting blindly.

From the repository root:

```sh
cargo test -p suzu
cargo build --release -p suzu
```

Do not require the Tauri Workbench to build on a headless host. Its GTK/WebKit
development dependencies are unrelated to the Resident.

## 3. Install from a checkout

Use the keeper name captured before escalation:

```sh
sudo ./scripts/install-linux.sh install --user KEEPER
```

The installer is idempotent. Its local-install layout is:

| Purpose | Path |
| --- | --- |
| Binary | `/usr/local/bin/suzu` |
| Read-only catalog and firmware | `/usr/local/share/suzu/` |
| Local unit template | `/etc/systemd/system/suzu@.service` |
| Local udev policy | `/etc/udev/rules.d/60-suzu.rules` |
| Keeper state, backups, captures | `/var/lib/suzu/KEEPER/` |

Use `--prefix /opt/suzu` only when local policy requires it; the generated
unit receives the same prefix. `--no-start` installs and enables the instance
without starting it.

## 4. Verify the installation

First verify files and service state:

```sh
./scripts/install-linux.sh verify --user KEEPER
systemctl status suzu@KEEPER.service --no-pager
systemctl show suzu@KEEPER.service \
  -p User -p SupplementaryGroups -p FragmentPath -p Environment
journalctl -u suzu@KEEPER.service -b --no-pager -n 100
```

Expected evidence:

- `Active: active (running)` and `User=KEEPER`;
- `SupplementaryGroups=suzu-hw`;
- a non-empty catalog loaded from the installed resource directory;
- `127.0.0.1:7899` listening, with no public bind;
- for a connected supported device: `sensed`, `identified`, an admission
  result, and `stream attached` in the journal.

Check the live API without installing a browser:

```sh
curl --max-time 3 -Ns http://127.0.0.1:7899/api/events | sed -n '1,3p' || true
```

On a minimal host without `curl`, use netcat:

```sh
timeout 5 sh -c "printf 'GET /api/events HTTP/1.1\r\nHost: 127.0.0.1\r\nAccept: text/event-stream\r\n\r\n' | nc 127.0.0.1 7899 | head -n 8" || true
```

The first server-sent event must be `event: snapshot`. Its JSON should name the
connected device. For serial permission failures, inspect the device rather
than adding the keeper to a broad login group:

```sh
udevadm info --query=property --name=/dev/ttyUSB0
stat -c '%A %U %G %n' /dev/ttyUSB0
```

A supported port should be group-owned by `suzu-hw` with mode `0660` (desktop
logins may additionally receive an ACL through `uaccess`). The service gets
that group at process start, so no logout/login cycle is required.

Finally test clean lifecycle behavior:

```sh
sudo systemctl restart suzu@KEEPER.service
sudo systemctl stop suzu@KEEPER.service
sudo systemctl start suzu@KEEPER.service
journalctl -u suzu@KEEPER.service -b --no-pager -n 40
```

Stop should log that shutdown was requested and that the Resident rests; it
must not require `SIGKILL`. Restart should reclaim the same device.

## 5. Native packages

Release artifacts should use native packages while retaining the same runtime
contract:

- Debian-family: `.deb`;
- Arch: `PKGBUILD`/package repository;
- Fedora-family: `.rpm`;
- generic fallback: a signed release archive plus this installer.

A native package owns `/usr/bin/suzu`, `/usr/share/suzu`, the unit under the
distro's systemd unit directory, and the udev rule under its vendor rules
directory. At package-build time replace `@SUZU_BINDIR@` and
`@SUZU_RESOURCE_DIR@` in `crates/suzu/deploy/suzu@.service` with `/usr/bin` and
`/usr/share/suzu`. Package scripts may create the `suzu-hw` system group and
reload systemd/udev, but should follow distro policy about automatically
enabling services. Enabling `suzu@KEEPER` remains the explicit machine-setup
step.

Never make mutable state package-owned. Upgrades replace the binary/resources
and restart enabled instances; they preserve `/var/lib/suzu`. This lets a GUI,
CLI, or agent report the same rollback boundary clearly.

## 6. Uninstall and rollback

For a checkout installation:

```sh
sudo ./scripts/install-linux.sh uninstall --user KEEPER
```

This stops and disables the instance, removes the files installed under the
chosen prefix, and preserves `/var/lib/suzu/KEEPER`. The shared `suzu-hw` group
is also preserved because another instance may use it. Only when the keeper
has confirmed backups and captures are disposable:

```sh
sudo ./scripts/install-linux.sh uninstall --user KEEPER --purge-state
```

State purge is irreversible from the installer. Native-package installs must
be removed with their package manager, not this fallback script.

## Agent completion report

An agent is finished only after reporting all of the following:

- distribution, architecture, init system, keeper, and installation method;
- exact installed binary version/path and unit path;
- active/enabled state and the service's effective user/groups;
- loopback API snapshot result;
- connected device identity and admission/stream result, or an explicit note
  that no supported device was present;
- whether state was created, preserved, migrated, or intentionally purged;
- any divergence from this playbook and why.

# OpenMicro

OpenMicro is an all-Rust control plane that turns an ESP32-based macropad into a
live status display and controller for your terminal AI agents. A background
daemon (`openmicrod`) tracks each agent session's state (idle / thinking /
working / awaiting-approval), drives the device's per-key LEDs to reflect them,
and routes physical key presses back to the focused session. A terminal UI and a
small CLI let you watch and configure everything from your shell.

The **host side is complete and works today** (against a mock transport and over
BLE). The **firmware is a separate, still-pending piece** — see
[Firmware status](#firmware-status) below.

## Components

- `openmicrod` — the daemon. Serves the hook socket (agents push state) and the
  control socket (clients read snapshots / send config commands).
- `openmicro` — the user-facing binary. With no arguments it launches the TUI;
  with a subcommand it acts as a CLI (see below).
- `openmicro-hook` — the shim agents call from their lifecycle hooks to report
  state transitions to the daemon.

## Install

From a checkout of this repo:

```sh
packaging/install.sh
```

This builds the release binaries, installs `openmicrod`, `openmicro`, and
`openmicro-hook` into `~/.local/bin`, and installs a systemd **user** service.
Then enable the daemon and open the UI:

```sh
systemctl --user enable --now openmicrod   # start now and on login
openmicro                                  # open the TUI
```

Make sure `~/.local/bin` is on your `PATH`. To keep the daemon running after you
log out, run `loginctl enable-linger "$USER"` once.

To remove everything: `packaging/uninstall.sh` (add `--purge` to also delete
`~/.config/openmicro`).

## CLI

Running `openmicro` with no subcommand opens the interactive TUI. The
subcommands are:

| Command | Description |
| ------- | ----------- |
| `openmicro status` | Print a one-shot summary of the daemon's state (agents, states, owner, battery). Exits non-zero with `daemon not running` if the daemon is down. |
| `openmicro service <start\|stop\|restart\|status\|enable\|disable>` | Wrapper around `systemctl --user <action> openmicrod.service`. |
| `openmicro install-agent <claude\|codex\|grok\|t3>` | Print setup instructions for an agent adapter. |
| `openmicro flash` | Firmware flashing (arrives with the firmware work; currently a pointer). |

## Agent setup

Each supported agent has an adapter under `adapters/`. To see the steps for one:

```sh
openmicro install-agent claude
```

This prints that adapter's `install.md`. The adapters wire an agent's lifecycle
hooks/notifications to `openmicro-hook`, which pushes state to the running
daemon. Hook-merge adapters (Claude Code, Grok, T3) are printed as instructions
rather than applied automatically, so nothing in your agent config is changed
without your say-so.

## Firmware status

The firmware that runs on the macropad is **separate from this host software and
is not yet ready** — it is awaiting hardware pinout confirmation and a flashing
path. Until then the daemon runs against a mock device (and can talk to a real
device over BLE where available). The current hardware investigation lives in
[`docs/hardware/creator-micro-2-pinout-research.md`](docs/hardware/creator-micro-2-pinout-research.md);
`openmicro flash` is a placeholder that will perform flashing once that work
lands.

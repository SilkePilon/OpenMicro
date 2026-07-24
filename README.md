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

## Setup wizard

The first time you run `openmicro`, it opens a guided wizard instead of the
dashboard. It walks the whole path from an untouched device to a working one:

1. **Custom-firmware warning.** Flashing OpenMicro is not an intended use of the
   Creator Micro 2. Going back to stock **is** possible, but only from a backup
   of your own device taken *before* flashing — the wizard offers to take it.
2. **Device detection.** It scans USB and Bluetooth with a live spinner until it
   finds the macropad, and reports what it found and how.
3. **Already on OpenMicro firmware?** It skips straight to agent setup.
4. **Reachable over Bluetooth only?** It asks for a USB cable — firmware can
   only be written over USB — and then for the boot button, polling until the
   device appears in ROM bootloader mode.
5. **Firmware.** Back up the stock image, **build** it from `firmware/` or
   **download** a prebuilt release, then flash. Actions you cannot run yet are
   greyed out with the reason (missing toolchain, no image, not in bootloader
   mode) rather than failing halfway.
6. **Agents.** It detects which coding agents are installed and installs their
   hooks for the ones you tick.

Re-run it any time with `openmicro setup`, or press `s` on the dashboard. It is
resumable and every step is skippable.

## CLI

Running `openmicro` with no subcommand opens the interactive TUI (the wizard on
first run, the dashboard afterwards). The subcommands are:

| Command | Description |
| ------- | ----------- |
| `openmicro status` | Print a one-shot summary of the daemon's state (agents, states, owner, battery). Exits non-zero with `daemon not running` if the daemon is down. |
| `openmicro service <start\|stop\|restart\|status\|enable\|disable>` | Wrapper around `systemctl --user <action> openmicrod.service`. |
| `openmicro setup` | Re-open the guided setup wizard. |
| `openmicro agents` | List which coding agents are installed here and whether their OpenMicro hooks are wired. |
| `openmicro install-agent <claude\|codex\|grok\|t3> [--print]` | Install that agent's hooks into its own config (`--print` shows the adapter docs instead). |
| `openmicro install-agent --all` | Install hooks for every detected agent that is missing them. |
| `openmicro firmware <build\|download\|status>` | Build the firmware from source, fetch a prebuilt release, or report which sources are available. |
| `openmicro flash [--image <path>] [--port <path>]` | Flash firmware to a Micro 2 in bootloader mode (see [Flash the firmware](#flash-the-firmware)). Stops with clear guidance and a non-zero exit if the image is missing, no flash tool is installed, or the device isn't in bootloader mode. |
| `openmicro backup [--out <path>]` | Dump the device's whole flash so the stock firmware can be restored later. |
| `openmicro restore [--image <path>]` | Write a saved stock-firmware backup back to the device. |

## Agent setup

Each supported agent has an adapter under `adapters/`. The wizard's agent screen
installs them for you; from the CLI:

```sh
openmicro agents                  # what's installed, and what's already wired
openmicro install-agent claude    # wire one agent
openmicro install-agent --all     # wire every detected agent
openmicro install-agent claude --print   # just show the adapter's install.md
```

Installing is **merge-only and idempotent**: your existing settings and hooks are
preserved (including key order), the previous file is copied to
`<name>.openmicro.bak`, the new file is written atomically, and re-running is a
no-op. If a config cannot be merged safely — invalid JSON, or a Codex `notify`
key already pointing somewhere else — it is reported and left untouched rather
than overwritten.

T3 Code has no hook API of its own (it drives other agents), so it is listed but
cannot be installed; wire the underlying Claude Code or Codex adapter instead.

The hooks call `openmicro-hook` by bare name, so it must be on the `PATH` your
agent runs with — the wizard and CLI warn when it is not.

## Firmware status

The firmware that runs on the macropad is **separate from this host software and
is not yet ready to flash** — it exists as a documented skeleton awaiting
hardware pinout confirmation and a build. What's done:

- **`crates/openmicro-effects`** — the LED effect resolver (Solid / Breath /
  Pulse / Rainbow) is complete, host-tested (`cargo test -p
  openmicro-effects`), `no_std`, and shared by the firmware unchanged.
- **`firmware/`** — an ESP32-S3 embedded skeleton (BLE GATT server, WS2812
  render loop, key/encoder/joystick input scanning) with every physical pin
  as a documented `// TODO(pinout):` placeholder. It is its own Cargo
  workspace so it never affects `cargo build/test/clippy --workspace` at the
  repo root. **It has not been compiled** — there is no Xtensa Rust toolchain
  in this environment; see [`firmware/README.md`](firmware/README.md) for the
  toolchain install, build, and flashing steps once you have the device and
  its pinout in hand.

Until firmware is built, flashed, and the pinout is filled in, the daemon runs
against a mock device (and can talk to a real device over BLE where
available). The hardware investigation lives in
[`docs/hardware/creator-micro-2-pinout-research.md`](docs/hardware/creator-micro-2-pinout-research.md).

## Flash the firmware

The easiest path is `openmicro setup`, which walks the whole thing. Underneath
it there are two other front-ends over the same engine: the `openmicro flash`
CLI and the TUI installer screen (press `f`). All three are honest about
prerequisites — they will **not** pretend to flash if something is missing.

Three things must be true before a flash can happen:

1. **You have a firmware image.** Either build it or download one:
   ```sh
   openmicro firmware build      # needs the Espressif Xtensa toolchain (espup)
   openmicro firmware download   # fetches a prebuilt release
   openmicro firmware status     # what's available here, and which image is used
   ```
   See [`firmware/README.md`](firmware/README.md) for the toolchain setup. The
   default build output is
   `firmware/target/xtensa-esp32s3-none-elf/release/openmicro-fw` and downloads
   land in `~/.cache/openmicro/firmware/`; pass a different one with `--image`.
   Set `OPENMICRO_FIRMWARE_URL` to download from a fork or a local `file://`.
2. **A flash tool is installed** and on your `PATH` (or in `~/.local/bin` /
   `~/.cargo/bin`). Which one depends on the image: a from-source build is an
   **ELF**, which needs `espflash` to derive the bootloader and partition table;
   a downloaded release is a **merged binary**, which `esptool` writes at `0x0`.
   ```sh
   cargo install espflash     # for from-source builds
   pip install esptool        # for merged release images, and for backup/restore
   ```
   OpenMicro picks the right one from the image's format and refuses to hand an
   ELF to `esptool` (that would write something the chip cannot boot).
3. **The device is in bootloader mode.** The Micro 2 uses native
   USB-Serial-JTAG with **no auto-reset**, so you must enter download mode by
   hand: **hold the boot button while plugging in the USB cable**, then release
   once it re-enumerates. (In bootloader mode it shows up as USB `303a:1001`;
   running normally it is `303a:8298`.)

Then flash from the CLI:

```sh
openmicro flash                 # auto-detects the image and device
openmicro flash --image path/to/fw.bin --port /dev/ttyACM0
```

or open the TUI (`openmicro`), press `f`, and follow the checklist — each item
shows ✓ or ✗ with what to fix, and once all three pass you can press Enter to
flash.

If a prerequisite is missing, every front-end prints exactly what to do and
exits non-zero; nothing is written to the device.

## Going back to the stock firmware

Reverting **is** possible, but it is not a supported use of the device and it
only works from a backup of *your own* device — there is no published stock
image to fall back on. Take the backup **before** you flash, with the device in
bootloader mode:

```sh
openmicro backup                # dumps the whole 4 MiB flash (needs esptool)
```

It lands in `~/.local/share/openmicro/stock-firmware.bin`. Keep it. To go back,
re-enter bootloader mode and:

```sh
openmicro restore
```

Without a backup, `restore` refuses rather than pretending it can help.

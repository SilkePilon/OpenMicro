# OpenMicro

Your macropad's keys light up with what your coding agents are doing.

Each agent session gets a key. It glows one colour while the agent is thinking,
another while it's running tools, and a third when it's stuck waiting on you —
so you can look away from the terminal and still know when you're needed. Press
the key to jump focus to that session.

Written in Rust, top to bottom: daemon, TUI, agent adapters, and the ESP32-S3
firmware.

**Where this is:** the host side works today. The firmware compiles from a
skeleton that still needs the device's GPIO pinout filled in, and has never been
flashed. See [Firmware](#firmware).

## How it works

```
coding agent  ──hook──▶  openmicro-hook  ──▶  openmicrod  ──BLE──▶  macropad
                                                  │
                                                  └──▶  openmicro (TUI / CLI)
```

Three binaries:

|                    |                                                                                                  |
| ------------------ | ------------------------------------------------------------------------------------------------ |
| `openmicrod`     | The daemon. Tracks session state, drives the LEDs, routes key presses back.                      |
| `openmicro`      | What you run. No arguments opens the TUI; with a subcommand it's a CLI.                          |
| `openmicro-hook` | A tiny shim your agent calls on each state change. Always exits 0, so it can't break your agent. |

Four states, and that's the whole vocabulary: `idle`, `thinking`, `working`,
`awaiting_approval`.

## Install

```sh
packaging/install.sh
```

Builds release binaries into `~/.local/bin` and installs a systemd **user**
service. Make sure `~/.local/bin` is on your `PATH`.

```sh
systemctl --user enable --now openmicrod   # start now, and on login
openmicro                                  # first run opens the setup wizard
```

If you want the daemon to survive logout, run `loginctl enable-linger "$USER"`
once.

Uninstall with `packaging/uninstall.sh`, or `--purge` to also drop
`~/.config/openmicro`.

## First run

`openmicro` opens a wizard the first time, and takes you from an untouched
device to a working one:

1. **The warning.** Flashing OpenMicro is not something the vendor supports.
   You can go back, but only from a backup of your own device taken beforehand —
   the wizard offers to take it.
2. **Finding the device.** It watches USB and Bluetooth until the macropad turns
   up, and tells you what it found.
3. If it's already running OpenMicro firmware, it skips ahead to step 5.
4. **Getting it into bootloader mode.** Firmware only goes over USB, so if the
   device is Bluetooth-only it asks for a cable first. Then it asks you to hold
   the boot button while replugging, and waits.
5. **Firmware.** Back up the stock image, pick a version to download or build
   one from source, then flash.
6. **Agents.** It finds the coding agents installed on your machine and wires up
   the ones you tick.

Anything it can't do yet is greyed out with the reason, rather than failing
partway through. Every step is skippable, and `openmicro setup` reopens it.

## Everyday use

Run `openmicro` for the dashboard: one row per live session, the focused one in
yellow, battery in the title bar.

| Key   |                                                                                                    |
| ----- | -------------------------------------------------------------------------------------------------- |
| `c` | Config panel: brightness, per-state colours, idle-sleep timeout. Changes apply live and are saved. |
| `f` | Flash screen.                                                                                      |
| `s` | Reopen the setup wizard.                                                                           |
| `q` | Quit.                                                                                              |

## Agents

Each supported agent has an adapter in [`adapters/`](adapters/).

| Agent                                         | Mechanism                            |
| --------------------------------------------- | ------------------------------------ |
| [Claude Code](adapters/claude-code/install.md) | Lifecycle hooks, event JSON on stdin |
| [Codex CLI](adapters/codex/install.md)         | The`notify` program                |
| [Grok Code](adapters/grok-code/install.md)     | Claude-compatible hooks              |

The wizard installs them, or from the shell:

```sh
openmicro agents                # what's installed here, and what's already wired
openmicro install-agent claude  # wire one
openmicro install-agent --all   # wire everything detected
```

Installing merges into the agent's own config file. Your existing keys and hooks
survive, key order included; the old file is copied to `<name>.openmicro.bak`;
the write is atomic; running it twice does nothing the second time. If a config
can't be merged safely — invalid JSON, or a Codex `notify` already pointing at
another program — OpenMicro says so and leaves the file alone.

Anything that can run a shell command can be wired up by hand:

```sh
openmicro-hook push --agent <name> --session <id> --state working
```

One catch: hooks call `openmicro-hook` by bare name, so it has to be on the
`PATH` your agent runs with. OpenMicro warns you when it isn't.

## Firmware

The firmware lives in [`firmware/`](firmware/) as its own Cargo workspace, so it
never interferes with `cargo build` at the repo root. It's an
esp-hal/Embassy/TrouBLE application: BLE GATT server, WS2812 render loop, key
and encoder and joystick scanning.

Two things are still outstanding. Every physical pin is a `// TODO(pinout):`
placeholder, because the Creator Micro 2's GPIO assignments aren't public — see
[the research notes](docs/hardware/creator-micro-2-pinout-research.md) and
[what&#39;s left](docs/hardware/NEXT-STEPS.md). And it has never been compiled, so
expect the first build to need fixes in the hardware glue. The LED effect code
([`crates/openmicro-effects`](crates/openmicro-effects)) is shared with the host
and is already tested, so that part isn't where the bugs will be.

Until then the daemon runs against a mock device. Set `transport = "ble"` in the
config once you have real firmware on the board.

### Getting an image

```sh
openmicro firmware list                       # published versions
openmicro firmware download                   # newest stable release
openmicro firmware download --version v1.2.0  # a specific one
openmicro firmware build                      # compile it yourself
openmicro firmware status                     # what's available on this machine
```

Releases are built by [`.github/workflows/firmware.yml`](.github/workflows/firmware.yml)
and attached to the GitHub release as `openmicro-fw.bin`. Downloads are cached in
`~/.cache/openmicro/firmware/`. Building needs the Espressif Xtensa toolchain
(`cargo install espup && espup install`); see
[`firmware/README.md`](firmware/README.md).

Point `OPENMICRO_FIRMWARE_URL` at any URL to bypass the release list entirely,
or `OPENMICRO_RELEASES_URL` at a fork's API endpoint.

### Flashing

`openmicro setup` walks you through it. Or press `f` in the TUI for a checklist,
or use the CLI:

```sh
openmicro flash
openmicro flash --image path/to/fw.bin --port /dev/ttyACM0
```

Three things have to be true first, and all three front-ends tell you which one
isn't:

1. **You have an image**, built or downloaded.
2. **A flash tool is installed.** Which one depends on the image. A source build
   is an ELF and needs `espflash` (`cargo install espflash`) to derive the
   bootloader and partition table. A downloaded release is already merged, and
   `esptool` (`pip install esptool`) writes it at `0x0`. OpenMicro picks from the
   file's format, and refuses to hand an ELF to `esptool` — that writes something
   the chip can't boot.
3. **The device is in bootloader mode.** There's no auto-reset on this board
   (native USB-Serial-JTAG), so you hold the boot button while plugging the cable
   in, and let go once it re-enumerates. Bootloader mode shows up on USB as
   `303a:1001`; running normally it's `303a:8298`.

Nothing gets written to the device if a step is missing.

### Going back to stock

Possible, but only from a backup of your own board — there's no published stock
image anywhere. Take it **before** you flash, with the device in bootloader mode:

```sh
openmicro backup    # dumps all 4 MiB to ~/.local/share/openmicro/
```

Keep that file. To go back, re-enter bootloader mode and run `openmicro restore`.
Without a backup, `restore` tells you it can't help rather than trying.

## CLI

```
openmicro                    open the TUI
openmicro status             one-shot summary of the daemon's state
openmicro service <action>   start | stop | restart | status | enable | disable
openmicro setup              reopen the setup wizard
openmicro agents             list detected agents and their hook status
openmicro install-agent ...  wire an agent's hooks (--all, --print)
openmicro firmware ...       list | download [--version] | build | status
openmicro flash              write firmware to a device in bootloader mode
openmicro backup             save the current flash
openmicro restore            write a saved backup back
```

## Configuration

`~/.config/openmicro/config.toml`, written by the TUI's config panel:

```toml
transport = "mock"     # or "ble"
brightness = 200       # 0-255
sleep_minutes = 3      # blank the LEDs after this long idle; 0 disables

[colors.idle]
r = 0
g = 0
b = 0
# ...and thinking / working / awaiting_approval
```

## Development

```sh
cargo test --workspace
cargo clippy --workspace --all-targets
```

The firmware is excluded from the root workspace on purpose; build it from
inside `firmware/` with the Xtensa toolchain active.

Layout: `crates/openmicro-proto` (shared types, `no_std`),
`crates/openmicro-effects` (LED effects, `no_std`, shared with the firmware),
`crates/openmicrod` (daemon), `crates/openmicro` (TUI and CLI),
`crates/openmicro-hook` (agent shim), `firmware/` (ESP32-S3), `adapters/` (per-agent
integration docs), `packaging/` (install scripts and the systemd unit).

## License

MIT.

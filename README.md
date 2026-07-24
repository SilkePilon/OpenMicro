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
                                                  └──▶  openmicro (the TUI)
```

Three binaries:

|                    |                                                                                                  |
| ------------------ | ------------------------------------------------------------------------------------------------ |
| `openmicrod`     | The daemon. Tracks session state, drives the LEDs, routes key presses back.                      |
| `openmicro`      | What you run. One command, no arguments, no subcommands: it opens a menu.                        |
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
openmicro                                  # opens the menu
```

If you want the daemon to survive logout, run `loginctl enable-linger "$USER"`
once.

Uninstall with `packaging/uninstall.sh`, or `--purge` to also drop
`~/.config/openmicro`.

## Using it

There is one command, and it takes no arguments:

```sh
openmicro
```

That opens a menu. Everything lives in there — setting the device up, flashing
firmware, wiring agents, controlling the service, watching activity, and
removing the lot. If the background service isn't running, it offers to start it
before showing you the menu.

**Set up my macropad** is the guided path, and does the whole job in order:

1. Explains that this replaces the vendor firmware, and that going back needs a
   backup you take first.
2. Finds the device over USB and Bluetooth.
3. Skips ahead if it's already running OpenMicro firmware.
4. Reboots it into bootloader mode — by asking the firmware, not by holding a
   button. See [Bootloader mode](#bootloader-mode).
5. Offers to back up the stock firmware, then downloads or builds an image and
   writes it.
6. Detects your coding agents and wires up the ones you tick.

The other entries do the same things individually: **Watch agent activity** is a
live view of every session and which key it's on; **Lights and sleep** changes
brightness, per-state colours and the idle timeout; **Firmware**, **Coding
agents**, **Background service** and **Device** each open their own submenu; and
**Uninstall OpenMicro** takes it back off the machine.

Esc backs out of any prompt and returns to the menu.

## Agents

Each supported agent has an adapter in [`adapters/`](adapters/).

| Agent                                         | Mechanism                            |
| --------------------------------------------- | ------------------------------------ |
| [Claude Code](adapters/claude-code/install.md) | Lifecycle hooks, event JSON on stdin |
| [Codex CLI](adapters/codex/install.md)        | The `notify` program                 |
| [Grok Code](adapters/grok-code/install.md)     | Claude-compatible hooks              |

**Coding agents** in the menu lists the ones found on this machine, pre-ticks
those that are installed but not yet wired, and installs the ones you pick.

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

It builds, and the GPIO map is real: Work Louder publish their firmware
unencrypted, so the pinout was recovered by disassembling it rather than by
opening the case ([findings](docs/hardware/creator-micro-2-pinout-findings.md)).
`pins.rs` asserts at compile time that no function collides with another or with
the flash, PSRAM and USB pins.

What it cannot do yet is drive the board: the HAL plumbing in `main.rs`'s task
bodies is still stubbed, the key-ID to matrix-coordinate map is unknown, and the
MAX77972 fuel gauge's I2C pins were the one thing the vendor image would not
give up statically. [What's left](docs/hardware/NEXT-STEPS.md) has the details.
The LED effect and WS2812 encoding code
([`crates/openmicro-effects`](crates/openmicro-effects)) is shared with the host
and unit-tested, so that part isn't where the bugs are.

Until then the daemon runs against a mock device. Set `transport = "ble"` in the
config once you have real firmware on the board.

### Getting an image

**Firmware → Get firmware** lists every published version with its date and
size and downloads the one you pick, or builds from source if you have the
toolchain. A release whose build produced no image is shown greyed out rather
than hidden.

Releases are built by [`.github/workflows/firmware.yml`](.github/workflows/firmware.yml)
and attached to the GitHub release as `openmicro-fw.bin`. Downloads are cached in
`~/.cache/openmicro/firmware/`. Building needs the Espressif Xtensa toolchain
(`cargo install espup && espup install`); see
[`firmware/README.md`](firmware/README.md).

Point `OPENMICRO_FIRMWARE_URL` at any URL to bypass the release list entirely,
or `OPENMICRO_RELEASES_URL` at a fork's API endpoint.

### Flashing

**Firmware → Flash the device** does it, rebooting into bootloader mode first if
it needs to. Two things have to be true, and the menu says which one isn't:

1. **You have an image**, downloaded or built.
2. **A flash tool is installed.** Which one depends on the image. A source build
   is an ELF and needs `espflash` (`cargo install espflash`) to derive the
   bootloader and partition table. A downloaded release is already merged, and
   `esptool` (`pip install esptool`) writes it at `0x0`. OpenMicro picks from the
   file's format, and refuses to hand an ELF to `esptool` — that writes something
   the chip can't boot.

Nothing gets written to the device if either is missing.

### Bootloader mode

You do not hold a button. The firmware enumerates as a composite HID device, so
the ESP32-S3's USB-Serial-JTAG peripheral isn't on the bus at all and esptool's
usual reset sequence has nothing to talk to. Instead OpenMicro asks the firmware
to reboot itself, the way Work Louder's own tooling does: a `sys.bootloader`
JSON-RPC call over the vendor HID interface (usage page `0xFF00`), framed into
64-byte reports.

Leaving bootloader mode is a separate mechanism. The force-download-boot bit in
`RTC_CNTL_OPTION1_REG` is battery-backed on the Pro and survives an unplug, so it
is cleared explicitly, and the reset is `--after watchdog-reset` — a plain reset
doesn't re-sample the boot straps on USB-Serial-JTAG, and there are no DTR/RTS
lines for the classic auto-reset.

Both are in the **Device** menu if you want them on their own. Connect the device
over USB with a data-capable cable; charge-only cables enumerate nothing. In
bootloader mode it shows up on USB as `303a:1001`; running its firmware it is
`303a:8297`, `303a:8298` or `303a:8360`.

### Going back to stock

Straightforward: Work Louder publish their firmware openly, as unencrypted
merged images. **Firmware → Restore the stock firmware** lists every published
vendor version and writes the one you pick. No backup required.

**Firmware → Back up the stock firmware** is still worth doing before you flash
if you want your device's *exact* image, settings and all, rather than a stock
one — it dumps all 4 MiB to `~/.local/share/openmicro/`, and restore offers it
alongside the vendor releases.

## Configuration

`~/.config/openmicro/config.toml`, written by **Lights and sleep** in the menu:

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
`crates/openmicrod` (daemon), `crates/openmicro` (the TUI),
`crates/openmicro-hook` (agent shim), `firmware/` (ESP32-S3), `adapters/` (per-agent
integration docs), `packaging/` (install scripts and the systemd unit).

## License

MIT.

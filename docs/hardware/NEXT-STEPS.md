# What's left for you (the physical hardware steps)

Everything that can be built and tested without the hardware loop is **done** (host software: daemon, TUI, adapters, packaging, effect core, installer — 118 tests, v1.1.0). Three things remain that need you physically present with the Creator Micro 2, because they involve the boot button and (possibly) opening the case, and a bad flash needs someone there to recover.

## Step 1 — Get the GPIO pinout (one-time)
The MCU is confirmed **ESP32-S3**, but no GPIO pin numbers are public (see `creator-micro-2-pinout-research.md`). Pick one:

- **A. Read-flash + disassemble (no case opening):** put the device in bootloader mode (hold the small boot button on the bottom while plugging in USB — it enumerates as `303a:1001`), then:
  ```
  esptool --chip esp32s3 read_flash 0x0 0x400000 stock-firmware.bin
  ```
  This is also your **safety backup** — keep it. Then disassemble/grep the binary for `gpio_`, `led_strip`/`rmt`, `adc`, `touch_pad` init to recover the pins. (I can help with the disassembly when you're back.)
- **B. Open the case + continuity-test** the PCB against the ESP32-S3 module pads for: key matrix rows/cols + diode direction, the WS2812 data pin + LED count/order, encoder A/B + press, joystick X/Y ADC (+ button), touch pin, battery-sense pin.

Fill the results into `firmware/src/pins.rs` (every unknown is a grouped `// TODO(pinout):` there — one file to edit).

## Steps 2 and 3 — just run the wizard
```
openmicro setup
```
The TUI now drives the rest end to end. It shows the custom-firmware warning (including that reverting is possible **only** from a backup you take first), watches USB and Bluetooth with a spinner until it finds the device, tells you exactly what to do next based on what it found — swap to a USB cable if it is only on Bluetooth, hold the power/boot button while replugging if it is cabled but running — and moves on by itself the moment the device shows up in bootloader mode.

Its firmware menu then does the remaining work, greying out anything it cannot do yet and saying why:

- **Back up the stock firmware** — do this first. (`openmicro backup` from the CLI.) It dumps all 4 MiB to `~/.local/share/openmicro/stock-firmware.bin` and is the only route back (`openmicro restore`).
- **Build firmware from source** — needs the Xtensa toolchain (`cargo install espup && espup install`; details in `firmware/README.md`). Equivalent to `openmicro firmware build`. Read "Where the firmware build stands" below first.
- **Download prebuilt firmware** — opens a picker listing every published version (`openmicro firmware list` / `openmicro firmware download [--version vX.Y.Z]` from the shell). The images come from `.github/workflows/firmware.yml`, which builds the firmware and attaches `openmicro-fw.bin` whenever you publish a release. No release exists yet, so this stays empty until you cut one — trigger the workflow manually ("Run workflow") first to see whether the firmware actually compiles.
- **Flash the device** — picks the right tool for the image: `espflash` for a from-source ELF, `esptool` for a merged release `.bin`.

Finally it asks which coding agents to wire up, detecting what is installed on this machine and merging the hooks into each agent's own config (backing the previous file up first). `openmicro agents` and `openmicro install-agent --all` do the same from the CLI.

After flashing, reset the device, set `transport = "ble"` in `~/.config/openmicro/config.toml`, and start the daemon (`systemctl --user enable --now openmicrod`). Your agent keys should then light up with live state.

## Where the firmware build stands

The firmware has now been compiled for real, on CI (`.github/workflows/firmware.yml`,
run it from the Actions tab). The toolchain half works: espup installs, the
crates resolve, and hundreds of dependencies build. Three blockers were found
and fixed on the way:

1. The espup toolchain ships `rust-src` but **no precompiled `core` for
   xtensa-esp32s3-none-elf** — its `lib/rustlib` only has the host triple. Fixed
   by building the standard library from source (`[unstable] build-std` in
   `firmware/.cargo/config.toml`).
2. `esp-println` panics in its build script unless exactly one output backend is
   enabled. Now `jtag-serial`, since this board has no UART header.
3. `trouble-host` refuses to build without `central` or `peripheral`. Now
   `peripheral` + `gatt`.

**What is still broken:** `esp-hal-embassy 0.9.1` does not compile against the
`esp-hal` it resolves to (1.1.1). It imports `esp_hal::sync::Locked`, which no
longer exists, and `RawPriorityLimitedMutex` no longer implements `RawMutex`.
The version comments in `firmware/Cargo.toml` explain why each pin was chosen,
but that reasoning came from reading published manifests rather than from a
build — the resolved set is not actually mutually compatible.

Fixing it means picking a set of esp-hal / esp-hal-embassy / esp-radio /
trouble-host / embassy versions that agree, most easily by starting from a
current `esp-hal` example for the ESP32-S3 and matching its `Cargo.toml`. That
is bring-up work worth doing alongside the pinout, not before it — nothing is
flashable until `firmware/src/pins.rs` is filled in anyway.

None of this touches the host side: `openmicro-effects` is host-tested, and
`cargo test --workspace` at the repo root does not build the firmware at all.

## Known follow-ups (v1.2, not blocking)
- BLE auto-reconnect is wired but minimal (bounded 2s attempt, 5s cooldown) — a proper background supervisor is nicer.
- `Approve`/`Interrupt` device actions are logged but not yet executed per-agent (needs a per-agent control channel).
- Firmware `main.rs` task bodies are wired to the modules but only fully validated once you compile+flash.

# openmicro-firmware (ESP32-S3, Creator Micro 2)

**Status: skeleton, UNVERIFIED against a compiler.** This crate has never
been built. There is no Xtensa Rust toolchain installed in the environment
this was written in, and installing one (`espup`) was explicitly out of
scope for that work. Everything below — including the exact `esp-hal` /
`trouble-host` / `esp-radio` API calls in `src/*.rs` — is written to match
those crates' documented/published shapes as of the versions pinned in
`Cargo.toml`, but is **not proven to compile**. Building it, fixing
whatever the real APIs need adjusting, filling in the pinout, and flashing
it are the next steps — they need a human with the physical device.

This is a **separate Cargo workspace** (`[workspace] members = ["."]` in
`Cargo.toml`) on purpose: the root OpenMicro repo's
`cargo build|test|clippy --workspace` never discovers or builds this crate.

## Why the pins are all `// TODO(pinout):`

The Creator Micro 2's actual GPIO wiring (key matrix, RGB data line,
encoder, joystick ADC, touch, battery sense) is **not published anywhere**.
Every host-side artifact talks to the device over a logical HID/BLE
protocol that never surfaces physical pins. Full research writeup:
[`docs/hardware/creator-micro-2-pinout-research.md`](../docs/hardware/creator-micro-2-pinout-research.md).
`src/pins.rs` is the single file to edit once they're known — see below.

### How to recover the real pins (in order of effort)

1. **Open the case and continuity-test the PCB** against the ESP32-S3
   module's pads. Fastest, most reliable.
2. **Dump the stock firmware and disassemble it**: put the device in
   bootloader mode (see below), then
   `esptool --chip esp32s3 read-flash 0 <flash_size> stock.bin`, and
   string/disassembly-scan the app partition (offset `0x10000`) for
   `gpio_`/`esp_rom_gpio`, RMT/`led_strip` init, ADC, and `touch_pad`
   config calls — the pin numbers are baked into the binary even though
   the host protocol hides them.
3. Check Work Louder's FCC internal photos (search their FCC grantee ID on
   fccid.io) for a schematic or board photo — not located as of the
   research pass above.

### The TODO(pinout) groups (7, all in `src/pins.rs`)

1. Key matrix — row/col GPIOs (or direct-GPIO wiring) + diode direction.
2. RGB LED data GPIO, chip type (WS2812 assumed, SK6812 unconfirmed),
   exact chain length/order.
3. Rotary encoder — A/B GPIOs + press GPIO.
4. Joystick — ADC X/Y GPIOs (+ any button).
5. Capacitive touch sensor GPIO.
6. Battery sense/charger — ADC pin or fuel-gauge I2C pins (which one is
   unconfirmed).
7. Flash total size + any non-default strapping pins.

## Toolchain install (you have not done this yet)

ESP32-S3 is Xtensa, which upstream `rustc` does not target — you need the
Espressif fork, installed via `espup`:

```sh
cargo install espup --locked
espup install
source ~/export-esp.sh     # or add to your shell profile; re-source per shell
```

This registers an `esp` rustup toolchain (matching `rust-toolchain.toml`
here) and the Xtensa LLVM backend.

## Build

```sh
cd firmware
cargo build --release
```

`.cargo/config.toml` pins the target to `xtensa-esp32s3-none-elf` and adds
the `linkall.x` link script arg esp-hal expects. **This has not been run
successfully here** — expect to iterate on the exact `esp-hal-embassy` /
`esp-radio` / `trouble-host` call sites in `src/main.rs`, `src/ble.rs`, and
`src/leds.rs` against whatever those APIs actually look like when you
build; they were written from documentation, not a compiler.

## Turning the build into a flashable image + flashing

The Micro 2 enumerates as `0x303A:0x1001` ("Espressif USB JTAG/serial debug
unit") in bootloader mode, and as `0x303A:0x8298` (or `0x8297`) running the
stock app. It uses **native USB-Serial-JTAG with no DTR/RTS auto-reset**,
so `espflash`/`esptool` cannot auto-enter download mode — you must put it
in bootloader mode by hand (hold the boot button while plugging in USB;
release once enumerated).

```sh
cargo install espflash --locked
# with the device in bootloader mode:
espflash flash --release --monitor
```

`espflash` builds the ELF into a flashable image itself (no separate
`objcopy`/merge-bin step needed for a standard single-app image). Flash
layout (standard ESP32-S3 scheme, per the research doc): 2nd-stage
bootloader at `0x0`, partition table at `0x8000`, app at `0x10000`.

## Not built or flash-tested here

- No Xtensa toolchain was installed (explicitly out of scope for the work
  that produced this skeleton).
- No `cargo build`/`cargo check` was run against `xtensa-esp32s3-none-elf`.
- No device was flashed.

What *was* verified without the toolchain: `cargo fmt --check` in this
directory, and that the shared logic this firmware calls into
(`openmicro-effects`'s LED effect resolver, `openmicro-proto`'s wire types)
is fully unit-tested in the root workspace (`cargo test --workspace` from
the repo root).

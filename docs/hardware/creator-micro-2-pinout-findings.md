# Creator Micro 2 / Codex Micro (ESP32-S3) — GPIO pinout FINDINGS

Research date: 2026-07-24. This document supersedes the "everything is unknown"
conclusion in `creator-micro-2-pinout-research.md`. **The full GPIO map was
recovered by disassembling Work Louder's own official firmware image.**

## Headline

The vendor kit (`@worklouder/wl-device-kit`) fetches firmware from **public
GitHub releases** under the `worklouder` org. The Creator Micro 2 firmware repo
is **`worklouder/cm-v2-fw-releases`** and every release ships a full merged
flash image (`firmware_<ver>_merged.bin`). These images are **not encrypted**
and contain the ELF app-description struct, full symbol-bearing assert strings,
and inlined pin constants. Disassembling with the Xtensa toolchain that ships in
`~/.rustup/toolchains/esp/` gave exact, cross-checked pin numbers.

**Every pin below marked CONFIRMED was read directly out of the shipping
firmware** (either as the `pin_bit_mask` immediate fed to `gpio_config()`, the
integer immediate fed to Arduino `analogRead()`, or the argument to the WS2812
`make_strip()` helper), and in almost every case the *very next instructions*
call `gpio_isr_handler_add(static_cast<gpio_num_t>(PIN_xxx), …)` where `PIN_xxx`
is the human-readable macro name embedded in an assert string. That pairing —
"here is the pin bitmask, and here is the assert that literally spells the
peripheral's macro name" — is what makes these CONFIRMED rather than guessed.

---

## Provenance

| Item | Value |
|---|---|
| Firmware repo | `https://api.github.com/repos/worklouder/cm-v2-fw-releases/releases` |
| Analysed image | `firmware_v0.6.0-rc.6_merged.bin` (latest as of research date) |
| Download URL | `https://github.com/worklouder/cm-v2-fw-releases/releases/download/v0.6.0-rc.6/firmware_v0.6.0-rc.6_merged.bin` |
| merged.bin sha256 | `05d8d8ad6ecfb34fa20a62f6b1822e3d9b8b50a569f78af56b5200703c4066dc` |
| Project name | `cm-v2-fw` (from app-desc struct) |
| Build toolchain | PlatformIO + arduino-esp32 core, on top of **ESP-IDF 5.3.2** |
| Local working copy | `/home/silke/.claude/jobs/45491553/tmp/` (`app.bin` = extracted factory app, `seg3.ann.dis` = annotated disassembly) |

Repo→device mapping proof: `wl-device-kit` `WLRelease.getFirmwareRepo()` returns
`"cm-v2-fw-releases"` for `device_type == "creator_micro_v2"`
(`Gnome-Input/sidecar/vendor/wl-device-kit/dist/index.js`, function
`getFirmwareRepo`). PIDs `0x8297`/`0x8298` map to `creator_micro_v2` there.

> **Codex Micro (PID `0x8360`):** no separate public firmware repo was found
> (`project-2077-fw-releases`, `codex-*-fw-releases` all 404). The Codex Micro
> is almost certainly the *same PCB* as the Creator Micro 2 with re-skinned
> firmware, so the pinout below is the best available starting point for it too,
> but that hardware-identity assumption is **unverified**.

---

## MCU / board facts (CONFIRMED from the image)

- **ESP32-S3**, Chip ID 9 (from image header + `esptool image-info`).
- **16 MB flash**, DIO, 80 MHz (image header).
- ESP-IDF 5.3.2.250210; bootloader compiled `Jul 23 2026`.
- Partition table (parsed from the image @ `0x8000`):
  | Partition | Type/Subtype | Offset | Size |
  |---|---|---|---|
  | (2nd-stage bootloader) | — | `0x0000` | — |
  | (partition table) | — | `0x8000` | — |
  | `phy_init` | data/phy | `0xf000` | 4 KB |
  | `factory` | **app** | `0x10000` | 8 MB |
  | `nvs` | data/nvs | `0x810000` | 128 KB |
  | `fs` | data (subtype `0x82`, LittleFS) | `0x830000` | 2 MB |
  | `coredump` | data/coredump | `0xa30000` | 64 KB |
- **The module is NOT octal-PSRAM.** The firmware freely drives GPIO35/36/37 as
  ordinary GPIO (see layer-LED and top-board-power below), which is impossible on
  a module whose GPIO33–37 are wired to octal PSRAM. So this is a quad-PSRAM /
  no-PSRAM 16 MB module (WROOM-1-class or MINI-1-class). Confidence: high.

---

## 1. Key matrix — CONFIRMED (electrical roles INFERRED)

The matrix is **interrupt-driven**, 4×4 (13 of 16 positions populated).
`wl_keymatrix::setup_gpio()` (@ `0x42045f08`) reads two member pin arrays and a
setter (`@0x4200c864`) installs them:

```
this+36 = &{46,17,40,47}   count this+44 = 4   -> configured GPIO_MODE_OUTPUT (no ISR)
this+40 = &{13, 5,21, 1}   count this+45 = 4   -> GPIO_MODE_INPUT, pull-DOWN, INTR_ANYEDGE, ISR
```

Pin arrays live in rodata at `0x3c101400` (`2e 11 28 2f 0d 05 15 01` =
`46,17,40,47,13,5,21,1`), loaded together at `0x4200cb41`–`0x4200cb47`.

| Role (electrical, CONFIRMED) | GPIOs |
|---|---|
| **Drive lines** (push-pull outputs, scanned) | **46, 17, 40, 47** |
| **Sense lines** (inputs, internal pull-down, any-edge IRQ) | **13, 5, 21, 1** |

- **CONFIRMED:** which 8 pins form the matrix, and which 4 are outputs vs which 4
  are interrupt-driven inputs. Read straight from `gpio_config()` mode fields.
- **INFERRED (medium):** calling the drive lines "columns" and sense lines
  "rows" (v1 called the driven set columns). Electrically, current flows
  drive→sense through a pressed key, so **diodes are oriented anode-on-drive,
  cathode-on-sense** (drive line asserted HIGH pulls the sense line up against
  its pull-down). This is the opposite polarity convention from the usual
  pull-up/drive-low QMK matrix — worth re-checking on hardware.
- **UNKNOWN:** the logical key-ID ↔ (drive,sense) coordinate mapping (which
  physical key sits at each intersection). Recoverable from the firmware's
  keymap table with more work, or trivially by pressing keys on the real device
  and watching which (row,col) fires.

**Cheap verification:** continuity-buzz each switch to the S3 pads; confirm the
8 pins above. Confirm diode orientation with a diode-test meter across one
switch (drive pad = anode side).

## 2. RGB LEDs (WS2812, SPI-driven) — CONFIRMED

RGB is **not** RMT-driven; it uses ESP-IDF `led_strip` in **SPI backend** mode
(`led_strip_new_spi_device`). Helper
`make_strip(int gpio, uint8_t count, spi_host_device_t host, led_strip_t*&)`
(@ `0x4200a6d8`) is called twice (@ `0x4200a804`, `0x4200a810`):

| Chain | Data GPIO | LED count | SPI host | Purpose |
|---|---|---|---|---|
| Per-key RGB | **GPIO7** | **13** | host 1 | one LED per mechanical switch |
| Underglow | **GPIO6** | **8** | host 2 | underglow (matches v1's `led_count:8`) |

- **CONFIRMED:** data pins (7 and 6), counts (13 and 8), SPI-backend driver.
- **INFERRED (high):** chip = **WS2812-family**, color order **GRB** — the
  `led_strip` default; the config builds a `color_component_format` bitfield but
  the exact R/G/B ordering byte was not fully decoded. If colors look swapped in
  your firmware, try RGB vs GRB.
- **Note:** each chain is on its own SPI host, so per-key and underglow are
  independent buses — don't try to daisy-chain them.

**Cheap verification:** scope GPIO7 / GPIO6 for the ~800 kHz WS2812 waveform on
boot, or write one pixel and see which physical LED lights.

## 3. Layer / indicator LEDs (LEDC PWM) — CONFIRMED

Separate from the RGB: three single-colour LEDs on **LEDC PWM** channels
(`wl_io::init_leds_gpio` @ `0x42008a1c`, `ledc_channel_config` ×3). These are the
ESP32 equivalent of v1's `WORK_LOUDER_LED_PIN_1..3`.

| Channel | GPIO | source |
|---|---|---|
| LEDC ch1 | **GPIO35** | inline immediate `movi a8,35; s32i (gpio_num)` |
| LEDC ch2 | **GPIO45** | template struct @ rodata `0x3c1010cc` |
| LEDC ch3 | **GPIO48** | template struct @ rodata `0x3c1010ec` |

- **CONFIRMED:** all three.
- ⚠ **GPIO45 is an ESP32-S3 strapping pin** (VDD_SPI). Driving it as an LED
  output after boot is fine, but it is sampled at reset — do not hold it in a
  state that changes the flash voltage strap during power-up.

## 4. Rotary encoder — CONFIRMED

`wl_io::init_encoder_gpio` (@ `0x42008ec4`), all three inputs INTR_ANYEDGE, ISR
handlers named in adjacent assert strings:

| Signal | GPIO | proof |
|---|---|---|
| `PIN_ENC_A` | **GPIO12** | mask `0x1000`, ISR add `movi a10,12` → `encoder_rotate_irq_handler` |
| `PIN_ENC_B` | **GPIO11** | mask `0x800`, `movi a10,11` → `encoder_rotate_irq_handler` |
| `PIN_ENC_SWITCH` (dial press) | **GPIO4** | mask `0x10`, `movi a10,4` → `encoder_button_irq_handler` |

## 5. Planar joystick (ADC) — CONFIRMED

Read with Arduino `analogRead()` (12-bit, raw 0–4095) in a sampler @ `0x420091dc`:

| Axis | GPIO | ADC1 channel | note |
|---|---|---|---|
| X | **GPIO9** | ADC1_CH8 | value inverted in FW (`4095 - raw`) |
| Y | **GPIO10** | ADC1_CH9 | |

- **CONFIRMED:** both axes, both on **ADC1** (good — ADC1 is usable with BLE
  active; ADC2 is not).
- **UNKNOWN:** a dedicated joystick push-button pin. The sampler reads only X/Y;
  no digital joystick-button GPIO was found. The press is probably either one of
  the 13 matrix keys or a deflection/threshold event, not its own GPIO. Verify
  by pressing the stick and watching for any single-GPIO edge.

## 6. Capacitive touch — CONFIRMED (and it's NOT the ESP touch peripheral)

The "touch sensor" is an **external touch controller**; the S3 only reads its
open-drain interrupt/output line. `wl_io::init_touchpad_gpio` (@ `0x42008e63`):

| Signal | GPIO | proof |
|---|---|---|
| `PIN_TOUCH_OUT_L` (active-low touch output) | **GPIO14** | mask `0x4000`, INPUT + ANYEDGE, ISR add `movi a10,14` → `touchpad_irq_handler` |

- **CONFIRMED.** Do **not** try to use the ESP32-S3 `touch_pad` peripheral for
  this — it's a plain digital IRQ from an external chip (the "_L" suffix = active
  low). The controller itself likely sits on the I²C bus (see §8).

## 7. Misc I/O — CONFIRMED

| Signal | GPIO | function name | notes |
|---|---|---|---|
| `PIN_REAR_BTN` | **GPIO2** | `init_rear_button_gpio` | input, ANYEDGE, ISR `rear_button_irq_handler` |
| `PIN_USB_DETECT` | **GPIO42** | `init_usb_detect_gpio` | input, ANYEDGE, ISR `usb_detect_irq_handler` (VBUS present) |
| Top-board power | **GPIO36, 37, 38** | `init_top_board_power_gpio` | 3 outputs (mask high `0x70`), power-gate the upper PCB |
| Charge enable | **GPIO44** | `init_charge_enable_gpio` | INPUT_OUTPUT, mask high `0x1000` — **INFERRED, medium confidence** (store alignment was noisy) |
| USB D-/D+ | **GPIO19 / GPIO20** | (fixed by silicon) | native USB-Serial-JTAG; hard-wired, never in FW config |

## 8. Battery / charger — chip CONFIRMED, I²C pins UNKNOWN

- **CONFIRMED:** battery + charging are handled by a **Maxim MAX77972**
  (integrated charger **and** fuel-gauge) on **I²C**. Proof: strings
  `MAX77972: %s%s`, `power.max77972.register_dump`,
  `.../drivers/max77972/driver.cpp`, and a debug line
  `state=… vcell=… soc=… chgin=… cok=… full=…`. The host protocol's
  `batteryPercentage` / `isCharging` come from this chip. There is **no analog
  battery-sense pin** — do not look for an ADC divider.
- **UNKNOWN: the I²C SDA/SCL GPIOs.** The firmware talks to the MAX77972 through
  the Arduino `Wire` library; `TwoWire::begin(sda, scl, freq)` is dispatched
  virtually, so the pin immediates could not be pinned down statically in this
  pass. They are **not** any of the 25 pins already assigned above.
  Free/unassigned pins that are the realistic I²C candidates: **3, 8, 15, 16,
  18, 33, 34** (0 excluded as boot strap in practice). Common S3 defaults would
  be a pair like 8/18 or 15/16 — **treat as a guess.**

**Cheap verification:** power the board, run an I²C-scanner on the real device
(the MAX77972 answers at a known 7-bit address ~`0x69`), sweeping candidate SDA/SCL
pairs; or scope the two unassigned pins nearest the MAX77972 for I²C traffic.

---

## Consolidated pinout table

| GPIO | Function | Confidence |
|---|---|---|
| 1 | Matrix sense line | CONFIRMED |
| 2 | Rear button (IRQ) | CONFIRMED |
| 4 | Encoder switch (dial press) | CONFIRMED |
| 5 | Matrix sense line | CONFIRMED |
| 6 | Underglow WS2812 data (8 LEDs, SPI host 2) | CONFIRMED |
| 7 | Per-key WS2812 data (13 LEDs, SPI host 1) | CONFIRMED |
| 9 | Joystick X (ADC1_CH8, inverted) | CONFIRMED |
| 10 | Joystick Y (ADC1_CH9) | CONFIRMED |
| 11 | Encoder B | CONFIRMED |
| 12 | Encoder A | CONFIRMED |
| 13 | Matrix sense line | CONFIRMED |
| 14 | Touch controller IRQ (active-low) | CONFIRMED |
| 17 | Matrix drive line | CONFIRMED |
| 19 | USB D- | fixed by silicon |
| 20 | USB D+ | fixed by silicon |
| 21 | Matrix sense line | CONFIRMED |
| 35 | Layer/indicator LED 1 (LEDC PWM) | CONFIRMED |
| 36 | Top-board power (output) | CONFIRMED |
| 37 | Top-board power (output) | CONFIRMED |
| 38 | Top-board power (output) | CONFIRMED |
| 40 | Matrix drive line | CONFIRMED |
| 42 | USB detect / VBUS present (IRQ) | CONFIRMED |
| 44 | Charge enable (INPUT_OUTPUT) | INFERRED (medium) |
| 45 | Layer/indicator LED 2 (LEDC PWM) ⚠strap | CONFIRMED |
| 46 | Matrix drive line ⚠strap | CONFIRMED |
| 47 | Matrix drive line | CONFIRMED |
| 48 | Layer/indicator LED 3 (LEDC PWM) | CONFIRMED |
| SDA/SCL | MAX77972 charger/fuel-gauge I²C | UNKNOWN (candidates 3/8/15/16/18/33/34) |

Matrix sense = {1, 5, 13, 21}; matrix drive = {17, 40, 46, 47}.

## ⚠ ESP32-S3 pins to never repurpose (and how this board relates)

- **Flash SPI (GPIO26–32):** reserved on every S3 module. **None used here** —
  good.
- **Strapping pins (0, 3, 45, 46):** GPIO0 and GPIO3 are **free/unused** here.
  **GPIO45 (VDD_SPI) and GPIO46 are USED** (LED2 and a matrix drive line) — they
  are only sampled at reset, so post-boot use is legal, but if you rewrite the
  firmware, mirror the vendor's choices and avoid holding them in a
  strap-changing state during power-up.
- **USB (GPIO19/20):** native USB-Serial-JTAG; leave alone.
- No pin in the CONFIRMED set collides with another, and none lands on
  flash/PSRAM — the map is internally consistent.

## What is still open

1. **I²C SDA/SCL** for the MAX77972 (§8) — the only functional pins not
   recovered. Needs an on-device I²C scan or scope.
2. **Charge-enable = GPIO44** — INFERRED only; re-read with a cleaner
   disassembly or buzz it out.
3. **Diode orientation / row-vs-col naming** (§1) — electrical direction is
   inferred from the pull-down + push-pull scan; confirm with a diode tester.
4. **WS2812 exact color order** (GRB assumed).
5. **Logical key-ID → (drive,sense) map** — not extracted; get it by pressing
   keys, or by decoding the firmware keymap table.
6. **Codex Micro (PID 0x8360)** is assumed to share this PCB; unverified.

## How to reproduce / go further

```
# 1. list firmware releases
curl -s https://api.github.com/repos/worklouder/cm-v2-fw-releases/releases | jq '.[].assets[].browser_download_url'
# 2. download + inspect
esptool image-info firmware_v0.6.0-rc.6_merged.bin           # header, IDF ver, app-desc
#    app partition starts at file offset 0x10000; carve it out and image-info again
# 3. disassemble (toolchain already on this machine):
~/.rustup/toolchains/esp/xtensa-esp-elf/esp-15.2.0_20250920/xtensa-esp-elf/bin/xtensa-esp32s3-elf-objdump \
    -D -b binary -m xtensa --adjust-vma=<seg_load_addr> <segN.bin>
```
The technique that cracked it: split the app image into its load segments,
disassemble each at its true VMA, then resolve every `l32r` literal back to the
value/string it points at. The vendor left full assert strings in
(`gpio_isr_handler_add(static_cast<gpio_num_t>(PIN_ENC_A), …)`), so each pin's
`gpio_config` bitmask sits a few instructions away from an assert that names the
exact `PIN_*` macro — turning raw immediates into labelled, high-confidence pins.

---

## Bring-up log: the LEDs (2026-07-25)

A closed-loop rig was used for this: flash, capture a webcam frame, measure the
mean RGB of the keypad region, repeat. It removes the "does it look lit?"
ambiguity entirely.

**Control experiment — the vendor firmware on the same board, same camera:**

| Firmware | Keypad region mean RGB |
| --- | --- |
| Work Louder v0.6.0-rc.6 | ~(95, 92, 104) — bright, visibly cycling |
| OpenMicro (SPI + DMA) | ~(12, 11, 12) — flat across 8 frames, no change |

So the LEDs, the chains, the power gate and the wiring are all fine. Whatever is
wrong is in our output path, not the hardware. That is worth stating plainly,
because three earlier hypotheses were about the hardware and all three were
wrong.

**What has been fixed and verified along the way**

- The upper PCB is power-gated behind GPIO36/37/38, and the vendor drives them
  `37=1, 36=0, 38=1` — *not* all high. Without this the chains have no supply.
- The encoding now matches ESP-IDF's `led_strip` SPI backend exactly (2.5 MHz,
  three SPI bits per LED bit, `100`/`110`), rather than an invented in-spec one.
- The vendor sets `flags.with_dma = 1`; without DMA a 137-byte frame exceeds the
  64-byte SPI FIFO and is chunked, breaking the continuous bitstream.
- Colour order is GRB with three components, decoded from their
  `led_color_component_format_t`. This we already had right.

**Still not lighting, and what is ruled out**

The render loop runs (a 2 s serial heartbeat proves it), the SPI writes return
`Ok` (failures are now reported rather than swallowed), and the frame content is
correct (host-tested). So the fault is between the SPI peripheral and the LED
data pin.

**Next thing to try: RMT instead of SPI.** RMT is the peripheral built for this
and states its timing in nanoseconds rather than smuggling it through a clock
divider and a bit pattern. An attempt was started and reverted — esp-hal 1.1's
`Channel` type parameters and the `TxChannelCreator` trait import did not match
the doc example, and guessing at the API blind was not converging. Worth 20
minutes with the real rustdoc for esp-hal 1.1.1 open.

Failing that, the remaining SPI-side suspects are the clock source (the vendor
passes `clk_src = 4`, which may not be what esp-hal's default resolves to, so
the real output rate may not be 2.5 MHz) and whether esp-hal drives MOSI at all
without an assigned SCK pin.

### SOLVED (2026-07-25): the power-gate pins were inverted

**The LEDs work.** The cause was a misread of the vendor's top-board power
setter. Its arguments map `a3 -> GPIO36, a4 -> GPIO37, a5 -> GPIO38`, but they
were first read as `37/36/38`, so the values recovered from
`init_top_board_power_gpio` were applied to the wrong pins — driving the exact
inverse of the enable. Every LED write then landed on an unpowered board: the
peripheral reported success and nothing lit, which is indistinguishable from a
broken driver.

Found by sweeping all eight combinations on the real device while clocking out
full-white frames and measuring with a camera:

| combo | 36 | 37 | 38 | keypad brightness |
| --- | --- | --- | --- | --- |
| 0 | 0 | 0 | 0 | ~25 (dark) |
| **1** | **1** | **0** | **0** | **107, 97, 91 (lit)** |
| 2 | 0 | 1 | 0 | ~23 (dark) |

Colour cycling then confirmed GRB: red frames read (96,47,50), green (46,82,62),
blue (39,49,111).

The lesson worth keeping: three separate output paths (SPI+DMA, RMT, and
hand-bit-banged GPIO) were all "wrong" for the same reason, and none of them
were wrong at all. When several independent implementations fail identically,
suspect their shared precondition rather than the implementations.

The elimination log below is kept because everything in it is still true, and
because it is what eventually forced the question "what do all three share?".

### Elimination log — chasing the wrong layer

Measured with the webcam rig (keypad-region mean RGB): vendor ~(95,92,104),
ours ~(13,12,13) flat.

Ruled out, each with evidence:

| Hypothesis | Why it is not the cause |
| --- | --- |
| Firmware not running | 2 s serial heartbeat from inside the render loop |
| Frame content wrong | encoder is host-tested; matches IDF byte-for-byte |
| Encoding scheme wrong | IDF's `__led_strip_spi_bit` decoded from source: MSB-first, 3 SPI bits per colour bit, `100`/`110`. Identical to ours (a zero byte is `0x92,0x49,0x24` in both) |
| Clock wrong | vendor image stores `0x2625A0` = 2 500 000 Hz; we request the same |
| Missing DMA | added; vendor sets `flags.with_dma = 1` |
| Colour order | GRB, decoded from their `led_color_component_format_t` |
| SPI peripheral misconfigured | **RMT was tried as a completely independent path and is equally dark** |
| Power-gate pins released when `main` returns | `Output` guards now leaked with `mem::forget`; still dark |

That RMT and SPI fail identically is the most informative result: two unrelated
peripherals cannot both be generating a bad waveform, so the signal almost
certainly never reaches the LEDs.

**Where to look next**, in order:

1. Something else in the vendor's init enables the LED rail. `init_charge_enable_gpio`
   (GPIO44, inferred) and the top-board sequencing are untested; there may also be
   a load switch behind the MAX77972 on I2C.
2. Whether GPIO7/GPIO6 are the data lines on *this* board revision. They are
   CONFIRMED from the image, but confirming against the PCB with a scope would
   settle it — probe GPIO7 while running, and compare with the vendor firmware
   running. If the vendor toggles it and we do not, the pin is not being driven;
   if both toggle, the fault is downstream.
3. Whether the vendor's `make_strip` runs *after* something that we do first.

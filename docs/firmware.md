# Firmware internals

Everything in this document used to live as comments in `firmware/src/`. It is
the knowledge a maintainer needs and cannot recover from the code alone: GPIO
assignments recovered from the vendor's shipping firmware, polarities found by
experiment, protocol contracts shared with the host, and warnings earned the
hard way. The full pinout derivation (offsets, disassembly proofs) is in
`docs/hardware/creator-micro-2-pinout-findings.md`.

## Status and provenance

The crate builds for `xtensa-esp32s3-none-elf` on CI
(`.github/workflows/firmware.yml`) against the version set pinned in
`Cargo.toml`: esp-hal 1.1.1 + esp-rtos 0.3.0 + esp-radio 0.18.0 +
trouble-host 0.6.0 — the same set as the upstream `esp-hal-v1.1.1`
`bas_peripheral` BLE example. Historically the crate was "compiles on CI, never
run on hardware", but several facts below (the top-board power-gate levels, the
serial command feedback loop, the WS2812 latch behaviour in probe mode) were
observed on a real device, so parts have since run on hardware; per-item
confidence is noted throughout.

The GPIO map in `pins.rs` is real, recovered from Work Louder's own published
firmware: the vendor publishes unencrypted merged images to
`worklouder/cm-v2-fw-releases`, and disassembling one puts every
`gpio_config()` pin bitmask next to an assert string naming its `PIN_*` macro.
Nothing outside `pins.rs` should hardcode a GPIO number.

Two behaviours of the stock firmware are deliberately preserved:

- the **power button** — short press wakes, ~2 s turns off (gesture logic in
  `openmicro_effects::power`, which also adds a long hold as a bootloader
  escape hatch);
- **entering and leaving the ROM bootloader on command** (`bootloader.rs`).
  Unlike the vendor's firmware, this one exposes USB-Serial-JTAG, so the host
  can also reset it into download mode with esptool directly — that path keeps
  working even if this firmware is wedged.

## pins.rs — GPIO map

### Reserved pins

`RESERVED_GPIOS = [19, 20, 26, 27, 28, 29, 30, 31, 32]` must never be
repurposed: GPIO19/20 are the native USB D-/D+ lines; 26..=32 are the module's
SPI flash and PSRAM. This is kept as a real constant with a compile-time
`const` assertion (at the bottom of `pins.rs`) that no driven pin collides
with a reserved one and no GPIO is claimed twice — a future edit that puts,
say, the encoder on a flash pin fails the build instead of the hardware.
A wrong pin in this file drives an output into another output, which is why
unknowns are recorded as unknowns rather than plausible placeholders.

### Key matrix — CONFIRMED (4x4, 13 of 16 positions populated)

- Drive pins: GPIO **46, 17, 40, 47** (push-pull outputs, idle low; a scan
  asserts one at a time).
- Sense pins: GPIO **13, 5, 21, 1** (inputs with internal pull-**down** and,
  in the stock firmware, an any-edge interrupt).
- `KEY_COUNT = 13` mechanical switches fitted; 3 intersections unused.

The pull-down is the important detail: it **inverts the usual keyboard
convention**. Current flows drive → sense through a pressed key, so a key
reads as pressed when its drive line is asserted HIGH and the sense line
follows it up against the pull-down (`MATRIX_ACTIVE_HIGH = true`). Diodes are
therefore anode-on-drive, cathode-on-sense — INFERRED from the electrical
roles; worth a diode-test meter before trusting it.

UNKNOWN: which physical key sits at each (drive, sense) intersection. The
vendor's keymap table was not decoded. Until it is, the scanner reports the
flattened index `row * MATRIX_SENSE_COUNT + col` as the key id; pressing keys
on a real device and watching which pair fires settles the map.

### RGB chains — CONFIRMED (WS2812 family, two chains)

- Per-key chain: GPIO **7**, **13** LEDs.
- Underglow ring: GPIO **6**, **8** LEDs.
- `LED_COUNT = 21` total.

The stock firmware drives them via ESP-IDF `led_strip` in its SPI backend,
each chain on its own SPI host — they are independent buses and must **not**
be daisy-chained. Byte order is GRB (the `led_strip` default) — INFERRED with
high confidence; if colours come out swapped on real hardware, this is the
first thing to try changing.

### Layer / indicator LEDs — CONFIRMED

Three plain single-colour LEDs on LEDC PWM channels, separate from the RGB
chains: GPIO **35, 45, 48**. GPIO45 is a strapping pin (VDD_SPI): driving it
as an output after boot is fine — it is only sampled at reset — but nothing
may hold it during power-up.

### Rotary encoder — CONFIRMED

- A = GPIO **12**, B = GPIO **11**, switch = GPIO **4**.
- The stock firmware puts an any-edge interrupt on both A and B.
- `ENCODER_PRESS_KEY_ID = 0xFE`: the wire protocol has `Key`, `Encoder` and
  `Joystick` events but no dedicated dial-press variant, and adding one would
  ripple through the daemon's action router for an input whose behaviour is
  not designed yet. A reserved id well clear of the 13 real keys carries it in
  the meantime.

### Joystick — CONFIRMED (both axes on ADC1)

- X = GPIO **9** (ADC1_CH8), Y = GPIO **10** (ADC1_CH9).
- ADC1 specifically matters: **ADC2 is unusable while the radio is active**,
  and this firmware runs BLE continuously.
- `JOYSTICK_X_INVERTED = true`: the stock firmware reports X as `4095 - raw`;
  this firmware matches it so the host sees the same orientation whichever
  firmware is running.
- UNKNOWN: a dedicated joystick push-button. The vendor's sampler reads only
  X/Y, so the press is probably one of the 13 matrix keys or a deflection
  threshold rather than its own GPIO.

### Capacitive touch — CONFIRMED (external controller)

Active-low interrupt line on GPIO **14** (`TOUCH_ACTIVE_LOW = true`) from an
**external touch controller** that lives on I2C. Do **not** point the
ESP32-S3 `touch_pad` peripheral at this pin: it is a plain digital input.

### Misc I/O — CONFIRMED

- Rear power button: GPIO **2**. Active low (button shorts a pulled-up line to
  ground) — INFERRED: the vendor firmware only shows the pin as an any-edge
  input, not its rest level. If the device powers itself off the instant it
  boots, this is the polarity to flip.
- USB / VBUS-present detect: GPIO **42**, active low (VBUS pulls the line down
  through a divider; the firmware configures a pull-up) — INFERRED: if the
  device thinks it is offline while plugged in, flip this polarity.
- Charge enable: GPIO **44** — INFERRED (medium confidence); the store
  alignment around it was noisy in the disassembly, so verify before driving.

### Top-board power gate — CONFIRMED empirically

`TOP_BOARD_POWER = [(36, true), (37, false), (38, false)]` — i.e. GPIO36
driven HIGH, GPIO37 LOW, GPIO38 LOW.

Both LED chains live on the **upper PCB**, which is power-gated behind these
three pins. The levels were determined empirically, by sweeping all eight
combinations on the real device while clocking out full-white frames and
watching with a camera: `36=1, 37=0, 38=0` lights both chains; every other
combination leaves them dark.

This corrects an earlier misreading of the vendor's setter. Its arguments map
`a3 → GPIO36, a4 → GPIO37, a5 → GPIO38`, not `37/36/38` as first assumed, so
the values recovered from `init_top_board_power_gpio` were applied to the
wrong pins — driving the exact inverse of the enable.

The failure mode is silent and cost a night of debugging: with the gate wrong
the upper board has no supply, every LED write "succeeds" at the peripheral
level, and nothing lights — indistinguishable from a broken driver.

### Battery / charger — chip CONFIRMED, bus pins UNKNOWN

There is **no analog battery-sense pin**. Battery state and charging come from
a Maxim **MAX77972** combined charger + fuel gauge on I2C (expected 7-bit
address `0x69`), which is where the host protocol's percentage and charging
flag come from.

The SDA/SCL GPIOs could not be recovered statically: the vendor firmware
reaches the chip through Arduino's `Wire`, whose `begin(sda, scl, freq)` is
dispatched virtually. They are none of the pins assigned above; the realistic
candidates are **3, 8, 15, 16, 18, 33, 34** (`BATTERY_I2C_CANDIDATES`). An
I2C scan on a real device settles it in minutes. The pins are deliberately
`None` in `pins.rs` rather than a guessed pair — driving the wrong two pins
as an I2C bus is exactly the mistake the file exists to prevent. Until the
bus is known, `battery_task` reports nothing rather than inventing a reading.

### Flash — CONFIRMED from the vendor image header

16 MB, DIO, 80 MHz. The app partition sits at `0x10000`.

## main.rs — boot, tasks, render loop, serial protocol

### Task layout

Embassy tasks wired together with `embassy-sync` channels:

- `ble_task` — will own the TrouBLE GATT server (`ble.rs`); currently a
  placeholder idle loop. Intended to receive `LedFrame` writes from the host
  and drain `InputEvent`s to notify.
- `led_render_task` — every `leds::RENDER_PERIOD_MS`, resolves the latest
  `LedFrame` via `openmicro_effects::resolve` (the host-tested effect core)
  and pushes it out over RMT.
- `input_task` — scans the key matrix / encoder / joystick and pushes
  `InputEvent`s.
- `serial_command_task` / `serial_input_task` — the cable link (see below).
- `battery_task` — will read the MAX77972 over I2C; blocked on the unknown
  bus pins.
- `power_task` — debounces the rear power button and acts on the gesture.

Channels:

- `LED_FRAME_CHANNEL` (host → firmware, depth **2**): depth 2 so a fresh
  write can supersede one not yet rendered without blocking the sender. When
  the channel is full the *older* frame is dropped, never the newer one — an
  out-of-date display frame is never worth showing.
- `INPUT_EVENT_CHANNEL` (firmware → host, depth **8**): room to drain a burst
  of key presses / encoder ticks without the input task blocking.
- `BATTERY_CHANNEL` (depth 1): latest battery reading for the Battery
  Service.

`HEAP_SIZE = 64 KiB` is a placeholder, to be tuned once real memory pressure
is measured on hardware. The heap exists because `openmicro-proto`'s postcard
encode/decode uses `alloc::vec::Vec`, and TrouBLE/esp-radio may need heap for
connection bookkeeping.

### Boot sequence notes

- `use esp_backtrace as _;` registers esp-backtrace's `#[panic_handler]`
  (printing a backtrace via esp-println on panic). Required: `#![no_std]`
  binaries must supply their own panic handler, and nothing else references
  the crate — removing the import removes the handler.
- `esp_bootloader_esp_idf::esp_app_desc!()` embeds the ESP-IDF app descriptor
  that esp-hal 1.1's boot flow requires for the 2nd-stage bootloader /
  espflash to accept the image.
- Logging is initialised at a **fixed** level (`Info`), not
  `init_logger_from_env`: that reads `ESP_LOG` at compile time, and a missing
  or mis-plumbed value silently filters every message — indistinguishable
  from the firmware being dead.
- `bootloader::clear_force_download()` runs first thing (see the bootloader
  section for why).
- `esp_rtos::start(timg0.timer0, software_interrupt0)` provides both the
  embassy time driver/executor integration and the RTOS scheduler esp-radio
  0.18 requires. It replaces the former `esp_hal_embassy::init` —
  esp-hal-embassy is dead upstream past esp-hal 1.0.x (see Cargo.toml's
  rationale block). In esp-radio 0.18 there is no separate `esp_radio::init`
  step; the scheduler comes from `esp_rtos::start`, per the upstream
  `bas_peripheral` example.
- embassy-executor 0.10 reshaped spawning: `#[task]` functions return
  `Result<SpawnToken, SpawnError>` and `Spawner::spawn` takes the token. The
  only failure is pool exhaustion, which for these single-instance tasks
  would be a bug, hence the `expect`s.

### The top-board power hold — do not "fix" this

`main` creates the three power-gate `Output`s and then calls
`core::mem::forget` on the array. This is deliberate and load-bearing:
esp-hal's `Output` is a guard — dropping it releases the GPIO — and `main`
returns as soon as the tasks are spawned, which would cut power to the upper
board before a single LED frame is drawn. That exact failure was chased for
hours, with both SPI and RMT faithfully clocking data into an unpowered
board. The same applies to the USB-Serial-JTAG **TX half**: esp-println owns
TX through raw register writes, so the TX guard from `split()` is leaked with
`mem::forget` rather than dropped — dropping it would silence every log line.

### WS2812 output: RMT, not SPI

The LED chains are driven by two RMT channels (channel0 → GPIO7 per-key,
channel1 → GPIO6 underglow), **not** SPI. The SPI path was matched to
ESP-IDF's `led_strip` backend exactly — same 2.5 MHz clock, same
three-SPI-bits-per-LED-bit `100`/`110` encoding, same GRB order, same DMA —
and it accepted every transfer without error while lighting nothing, on a
board where the vendor firmware lights the same chains happily. RMT is the
peripheral built for this: the timing is stated in nanoseconds rather than
smuggled through a clock divider and a bit pattern.

RMT configuration:

- 80 MHz with divider 1 → 12.5 ns ticks, which resolves every WS2812 interval
  exactly. The `ticks(ns)` helper is written as `ns * 8 / 100` so the timing
  constants stay expressed in nanoseconds.
- WS2812B datasheet intervals: T0H = 400 ns, T0L = 850 ns, T1H = 800 ns,
  T1L = 450 ns.
- Wire order per pixel is **green, red, blue**, MSB first.
- An all-zero pulse code ends the sequence; the channel is configured to
  idle low (`with_idle_output(true)`, level Low), so the line then holds down
  and that is the inter-frame latch.
- Gamma-encoding (`openmicro_effects::gamma`) is applied in `write_pixels`
  and **nowhere else**: it is the single output boundary, so every animation
  gets it exactly once. Without it a linear crossfade between adjacent LEDs
  looks like a snap, which reads as a choppy comet however smooth the
  arithmetic is.
- Pulse-code buffer sizes are `LED_COUNT * 24 + 1` (24 bits per pixel plus
  the terminator).

### Render loop

- Uses a `Ticker`, not `Timer::after` at the end of the loop: `after` sleeps
  for a fixed time *after* the work finishes, so the frame interval becomes
  16 ms plus however long rendering took — a blocking USB-serial write or a
  longer RMT transfer then shows as a visible hitch. A ticker holds the
  phase.
- The last host frame is *held* so the animation keeps running between
  heartbeats instead of freezing for a second and a half at a time.
- On each tick the channel is drained taking only the newest frame; a frame
  differing from `LedFrame::BLANK` counts as activity for the idle timer.
- USB-detect (GPIO42) feeds `openmicro_effects::status::link_state`, which is
  how the device distinguishes "no daemon running" (a host is there, so
  amber, fixable — `Link::NoDaemon`) from "no host at all" (aurora, just a
  fact — `Link::Offline`). The link-state decision itself lives in
  `openmicro_effects` on the host side of the fence so it is unit-tested
  rather than merely asserted.
- `Link::Live`: render the held host frame. `NoDaemon`/`Offline`: keys blank
  (no agent state means no agent colours) but the ring must still show
  *something*, because a dark board is indistinguishable from a broken one.
- The boot animation (`openmicro_effects::startup`) doubles as a wiring test:
  a sweep makes a wrong chain length, order or colour order obvious, where a
  static colour would hide all three.
- Heartbeat: a log line every 2000 ms (`alive t=..ms link=..`).
  USB-Serial-JTAG throws away anything written while no host is listening, so
  a one-shot boot message is invisible unless a monitor happened to be
  attached at exactly the right moment; a periodic line makes "is it alive?"
  answerable at any time.

### Idle sleep

- `LED_SLEEP_MS = 5 * 60 * 1000` (5 minutes): how long the device may sit
  untouched before the LEDs are blanked. WS2812s held at a constant colour
  for days is how panels develop uneven ageing, and this device lives on a
  desk. Any input wakes it.
- `LAST_ACTIVITY_MS` is written by the input and power tasks
  (`touch_activity`), read by the render loop. Only *real* input events count
  as activity — the matrix scan runs continuously and would otherwise keep
  the LEDs awake forever.
- While asleep the chains are blanked every frame; that is harmless (each
  frame is identical) and keeps the wake path a single comparison.
- `LAST_FRAME_MS` starts at zero, which reads as "a frame arrived at t=0" —
  but the boot animation covers the first `startup::DURATION_MS`, and by the
  time it ends the gap already exceeds `DAEMON_TIMEOUT_MS`, so the device
  correctly settles into its own animation if nothing is talking to it.
- `LOCAL_BRIGHTNESS = 255` is the brightness for the animations the device
  runs on its own. It is *not* the user's configured brightness: that arrives
  with a frame, and these are exactly the states where no frame is arriving.

### Display modes

Runtime state (`DISPLAY_MODE`, an `AtomicU8`) rather than a build flag, so
the TUI can switch modes on a device that is already flashed — asking someone
to rebuild firmware to see a demo is not a feature. Byte values are a
contract with the host:

| Value | Name | Purpose |
|-------|------|---------|
| 0 | `MODE_NORMAL` | Real state: host frames, or local fallback animations. |
| 1 | `MODE_DEMO` | Walk every state the board can show (`openmicro_effects::demo`). |
| 2 | `MODE_IDENTIFY` | Light one chain position at a time (dwell `IDENTIFY_DWELL_MS = 1500` ms), logging its index, to establish `layout::LED_FOR_KEY`. Walks the ring in step. |
| 3 | `MODE_PROBE` | Light three known chain positions — index 0 red, middle (`PER_KEY_LED_COUNT / 2`) green, last blue, plus ring index 0 white — and hold them. |

Probe exists because it is faster than identify for the question that
actually matters — which end of the chain is the top of the board: it can be
answered by looking once instead of watching thirteen steps go by. Whichever
physical row the blue LED is in tells you which end of the chain is which,
and that is the only thing standing between `LED_FOR_KEY` being identity and
being correct.

The mode at boot is seeded by compile-time env flags (`OPENMICRO_DEMO`,
`OPENMICRO_IDENTIFY`) but the serial commands are the real interface, and a
reset always returns to normal. `OPENMICRO_TRACE_RX` enables logging of every
byte arriving on the cable (`TRACE_CABLE_BYTES`).

### Serial command protocol — the `!` prefix (critical)

`COMMAND_PREFIX = b'!'`. Commands over USB-Serial-JTAG must be introduced by
this byte, and it **must match `COMMAND_PREFIX` in
`crates/openmicro/src/display.rs`** on the host.

Why the prefix exists: without it the board reprograms itself. This console
carries the firmware's log output *and* its command input, and a host tty in
its default line discipline echoes everything it receives straight back out —
so every line printed by the firmware arrived back as input. With bare
letters as commands that is a feedback loop: `link:` contains `i` (identify)
and `n` (normal), and any line with a `d` in it put the board into demo mode
on its own. This was observed on hardware, which is how it was found. `!`
never appears in this firmware's output, so the loop cannot close.

Parser rules (`handle_serial_command`):

- `armed` (one `bool`) is the whole parser state: the prefix sets it, and the
  very next byte is taken as the command. It is shared across FIFO drains — a
  prefix arriving at the end of one chunk must still arm the command at the
  start of the next.
- Bytes arriving unarmed are dropped **silently** — they are almost certainly
  this firmware's own output echoed back, and replying would print more
  output to echo. Do not add an "unknown byte" log for unarmed bytes.
- Commands: `!n`/`!N` normal, `!d`/`!D` demo, `!i`/`!I` identify, `!p`/`!P`
  probe, `!?` identify-the-device. Unknown *armed* bytes get a
  `mode: unknown command ...` reply; mode changes log `mode: <name>`.

`!?` replies with the `IDENTITY` banner: the exact string
`"openmicro-fw"` followed by a space and `CARGO_PKG_VERSION`. **The host
matches on this exact prefix.** It exists because USB PID `303a:1001` is
*both* the ESP32-S3 ROM bootloader and any firmware exposing USB-Serial-JTAG
— ours does — so the USB id alone cannot tell "running" from "in the
bootloader". The ROM bootloader answers nothing, which is the distinguishing
fact.

### Cable transport (`serial_command_task`) — the transport that works today

The BLE GATT server is still a sketch, so the cable is how the daemon drives
the display — and it is the more reliable of the two anyway: no pairing,
cannot drop out mid-session.

Two kinds of traffic share the RX stream:

- prefixed ASCII pairs are display-mode commands (`!d`/`!i`/`!p`/`!n`/`!?`);
- `wire`-framed binary (`openmicro_proto::wire`) is a postcard-encoded
  `LedFrame`.

They cannot be confused: the frame marker `0xF5` is not a legal UTF-8 byte,
and the mode commands are all ASCII. A byte is only considered as a command
if the wire reader is **not** mid-frame *and* the byte is ASCII — without
both checks every frame-payload byte would also be parsed as a command.
Framing bytes never reach the command parser because the reader consumes
them.

FIFO discipline, learned from a real data-loss bug:

- `RX_CHUNK = 64`: the peripheral's OUT endpoint is 64 bytes, so one
  `drain_rx_fifo` pass empties it completely.
- **Empty the FIFO first, then process.** Byte-at-a-time with work in between
  loses data: an earlier version printed a log line from inside the drain
  loop, and a framed packet arrived as its first byte and nothing else.
  Writing to the TX FIFO while the OUT endpoint still holds unread bytes
  costs enough time to lose them.
- Poll every **5 ms**, not the 50 ms a mode-switch alone would justify: the
  RX FIFO is 64 bytes and a framed `LedFrame` is around 50, so the drain
  must not fall behind.

Outbound (`serial_input_task`): input events are postcard-encoded,
`wire`-framed (buffer `wire::MAX_PAYLOAD + 3`), and written via
`esp_println::Printer::write_bytes` — raw bytes through esp-println's writer,
which owns the TX FIFO. Not `write!`: formatting would mangle the binary
payload. The framing lets the daemon pick events out of the same stream that
carries the logs. This task runs regardless of which transport the host uses.

### Input task details

- Matrix scan: assert one drive line at a time, wait **20 µs** for the line
  to settle before sampling (with a pull-down and any trace capacitance the
  first read after a transition is a lie), read all sense lines, deassert.
  Polarity per the vendor: pressed reads HIGH (see pins section).
- Scan loop period: 2 ms.
- Events are logged as well as queued: until the BLE link exists, the serial
  console is the only way to see that a key press was detected — and it is
  what identifies the key-ID map.
- Encoder: quadrature state packed as `(A << 1) | B`, seeded from the pins at
  startup so the first real edge is not read as a phantom step. Every
  transition is decoded (not just detents) so a fast twist does not drop
  steps. The encoder switch is active low (pull-up, switch shorts to
  ground).
- Joystick: sampled every `JOYSTICK_PERIOD_MS = 40` ms — it is a menu
  selector, not a pointer, so there is nothing to gain from 500 Hz. ADC full
  scale `ADC_MAX = 4095` (12-bit), nominal centre `ADC_CENTRE = 2048`.
  `JOYSTICK_DEADZONE = 700` is a first guess, not calibrated against a real
  stick — wants tuning on hardware. X is inverted
  (`ADC_MAX - raw`) to match the vendor firmware's reporting.

### Power task

Polls the rear button every 10 ms and feeds
`openmicro_effects::power::PowerButton` (host-tested gesture recognition; all
the task owes it is a debounced level and a clock). Actions: `EnterBootloader`
→ `bootloader::reboot_to_bootloader()`; `Wake` → activity touch; `PowerOff` is
not yet implemented (planned: blank both LED chains, then deep sleep with the
button as the wake source). Button polarity is inferred active-low — see the
pins section.

### Battery task

Currently sleeps forever: blocked on the unknown I2C bus pins (see the pins
section). It deliberately reports nothing rather than inventing a reading.

## leds.rs — chain buffers and remap

| Chain | GPIO | LEDs | Shows |
|-----------|------|------|------------------------------------------|
| Per-key | 7 | 13 | which agent, and what it is doing |
| Underglow | 6 | 8 | overall status, readable across the room |

This module decides *nothing* about colour or motion: those come from
`openmicro_effects::status` and `::ring`, which the daemon also uses, so the
two ends cannot drift. It only owns pixel buffers, the key-to-chain remap,
and the fallbacks for when the host has gone quiet. The bit-level encoding is
host-tested (`openmicro_effects::ws2812` for the SPI path) or done by RMT in
`main.rs`.

- `RENDER_PERIOD_MS = 16` (~60 Hz): the travelling motions need this to glide
  rather than step; slower and `Motion::Spin` visibly stutters.
- `KeyChain::set_key` is the **only** place the `layout::LED_FOR_KEY` remap
  is applied, so a wrong table shows up as the whole board being permuted
  rather than as one subsystem disagreeing with another.
- `set_chain_index` / `set_chain_indices` take *raw chain indices* on purpose
  — going through the remap would assume the very thing the bring-up modes
  are trying to measure.
- `set_chain_indices` does **one flush for the whole set**, which matters
  more than it looks: a WS2812 latches only after the data line is held low
  for ~50 µs, and building the next frame takes less than that. Flushing once
  per position had the strip treat the later writes as continuation data for
  LEDs that do not exist, so only the *first* write ever appeared — an
  earlier three-colour probe showed exactly one lit LED and nearly sent the
  author chasing a dead chain.
- `PixelOut` exists so the render logic never names esp-hal's RMT or SPI
  types (which are parameterised by peripheral instance and DMA channel).
  Write errors are dropped by callers: a lost frame is replaced by the next
  one 16 ms later, and there is nothing useful to do about it on a device
  with no display.
- `GlowRing` is a separate type from `KeyChain`, not a generic over length,
  because it is a different peripheral *and* a different concept: it shows
  device status, not agent state, and it is the one thing that keeps working
  when the host does not. `render_link` is the whole reason the firmware
  carries a copy of the status design: when the daemon is the thing that is
  missing, it cannot be the one to say so.

## input.rs — pure input logic

The logic is deliberately pure — debounce, quadrature decode, joystick
sectoring — taking already-sampled values so it does not depend on the HAL
and stays testable in spirit. The GPIO driving/reading happens in the embassy
input task in `main.rs`.

- `DEBOUNCE_MS = 10`: 8–12 ms is typical for mechanical switches; not derived
  from real hardware measurements. Debounce is per-intersection: a change
  must hold for the window before an event is emitted;
  `pending_since_ms` uses `now_ms.max(1)` because 0 means "no pending
  change".
- `encoder_step` is the standard 2-bit gray-code quadrature transition table:
  `00→01→11→10→00` is +1 per transition, the reverse is −1, anything else
  (skips, bounces) is 0.
- `joystick_to_sector` converts raw 12-bit X/Y to an 8-way sector index
  (0=N, 1=NE, 2=E, 3=SE, 4=S, 5=SW, 6=W, 7=NW; screen-style Y axis: dy > 0
  means "down"/S). Inside the deadzone (per-axis) no event is produced. The
  diagonal test avoids atan2/floats: each 45° sector boundary is
  tan(22.5°) ≈ 0.4142, approximated as the fixed-point fraction 27/64
  ≈ 0.4219 (`small * 64 >= large * 27`) — close enough for an 8-way menu
  selection.
- Protocol note: the wire protocol only has `InputEvent::Joystick { dir: u8 }`
  today. The research doc records that the real device exposes polar
  angle/distance to a 7-slot radial menu upstream, so the 8-way sector is a
  firmware-side approximation until `openmicro-proto` is extended to carry
  angle+distance directly.

## bootloader.rs — ROM download mode

The stock firmware exposes bootloader entry as a `sys.bootloader` JSON-RPC
over its vendor HID interface, because it enumerates as USB-OTG HID and never
puts USB-Serial-JTAG on the bus — leaving esptool nothing to talk to.
OpenMicro keeps the capability by two routes:

- **From the host, without firmware involvement**: this firmware logs over
  USB-Serial-JTAG, so that peripheral *is* on the bus and esptool can reset
  the chip into download mode itself (`--before usb-reset`). Works even if
  the firmware has crashed.
- **From the device**: `reboot_to_bootloader()`, reachable by holding the
  power button (see `openmicro_effects::power`) or by a host command.

Mechanism: `RTC_CNTL_OPTION1_REG` at `0x6000_812C`; setting its low bit
(`FORCE_DOWNLOAD_BOOT = 1 << 0`) tells the ROM to enter download mode on the
next boot instead of running the app, then a software reset. Both routes end
in the same place the vendor's does.

Two important properties of that bit:

- It **survives a reset**, and on the Pro model the RTC domain is
  battery-backed so it survives losing USB power too. This is why
  `clear_force_download()` runs first thing on every normal boot (and why the
  host clears it on the way *out* of download mode — see the host's
  `wldevice::exit_args`): left set, the device would re-enter download mode
  on every boot, including after an unplug.
- `reboot_to_bootloader` never returns and is an immediate reset, not a
  graceful shutdown — the caller must persist anything that matters first.

Safety argument for the raw register access: a single 32-bit volatile
read/write to a fixed peripheral register the ROM defines for exactly this
purpose; nothing else in the firmware touches it, and the write in the reboot
path is immediately followed by a reset, so there is no ordering hazard.

## ble.rs — GATT server sketch (UNVERIFIED)

Structural skeleton only. The intended shape: the custom OpenMicro service
(LED write characteristic + INPUT notify characteristic) plus the standard
Battery Service, over TrouBLE (`trouble-host`) on `esp-radio`'s BLE
controller (`BleConnector`, wrapped in `ExternalController`). The exact
trouble-host 0.6.0 GATT-server builder API usage (attribute-table macros,
`GattServer` construction, advertising, connection event loop) is written to
the shape documented in the TrouBLE examples but has not been exercised.

**Version pin that must not be bumped casually**: `trouble-host` is pinned to
0.6.0, not 0.7.0, because 0.7.0 requires `bt-hci ^0.9` while `esp-radio`
0.18.0 (our controller) still pins `bt-hci ^0.8.0` — confirmed incompatible
from both crates' published `Cargo.toml`. 0.6.0 requires `bt-hci ^0.8`,
matching esp-radio exactly.

Constants and rationale:

- UUIDs and the advertising-name prefix are re-exported from
  `openmicro_proto::ble` so host and firmware can never drift.
- `MAX_CONNECTIONS = 1`: the device only needs to serve one host at a time;
  TrouBLE requires this as a const generic on the stack.
- `MAX_ATTRIBUTES = 16`: service + 2 custom characteristics + battery
  service/char + the mandatory GAP/GATT service attributes, with headroom.
- `L2CAP_MTU = 247`: the common "fits one radio packet after ATT/L2CAP
  overhead" size; comfortably fits an encoded `LedFrame` (6 slots via
  postcard, well under 100 bytes) or `InputEvent` (a few bytes) in one
  write/notify.
- `decode_led_write` mirrors the daemon's encode step so a malformed or short
  write is a clear, named failure mode in the BLE task's logs rather than an
  anonymous decode error at the call site.
- `encode_input_notify` returns `None` on the practically unreachable
  postcard encode failure so the caller skips a bad notification instead of
  panicking the input task.

The original file carried a commented-out sketch of the eventual server
wiring: a `#[gatt_server]` struct with the OpenMicro service
(`led: [u8; 64]` write characteristic, `input: [u8; 32]` notify
characteristic) and a battery service (`level: u8`, read + notify);
`HostResources<MAX_CONNECTIONS, MAX_ATTRIBUTES, L2CAP_MTU>`;
`GapConfig::peripheral` with `ADV_NAME_PREFIX` and a generic-keyboard
appearance; a loop that advertises, accepts a connection, forwards decoded
LED writes to the render task's channel, and notifies input events drained
from the input channel.

`ble_task` in `main.rs` is still an idle placeholder. An open design note:
whether the BLE task forwards frames via a second channel or shares
`LED_FRAME_CHANNEL` as the single source of truth is TBD until the
attribute-write callback shape is known. `_RESOURCES_CELL` (a `StaticCell` in
`main.rs`) is a visible placeholder for structures the real implementation
will need to `'static`-promote, e.g. `HostResources`.

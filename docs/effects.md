# openmicro-effects — LED animation design and rationale

`openmicro-effects` is the host-testable LED effect resolver shared by the
firmware and (future) daemon preview code. It is `#![no_std]`, has no HAL
dependencies and no `unsafe`, and every animation is a **pure function of a
millisecond timestamp** (plus, for spatial effects, an LED index) — fully
deterministic and unit-testable without a clock or hardware. Tests pull in
`std` explicitly under `#[cfg(test)]`. All arithmetic is integer-only; nothing
allocates.

The render clock is a `u32` of milliseconds, which wraps every ~49.7 days —
acceptable for a render clock, but it means downstream code must treat `t_ms`
as wrapping (see `power.rs` and `demo.rs`, which both defend against it).

## lib.rs — per-key effect resolution and colour maths

`resolve(slot, t_ms)` turns a single `LedSlot` into a concrete `Rgb`. A slot
with brightness 0 is always black, regardless of effect.

Effect timing constants:

- **Breath**: slow, smooth triangle-wave brightness modulation.
  `BREATH_PERIOD_MS = 3000`, floor `BREATH_MIN_PCT = 15` % of the slot's
  brightness (the trough is dim but never fully off).
- **Pulse**: faster and sharper. `PULSE_PERIOD_MS = 700`, floor
  `PULSE_MIN_PCT = 10` %. Pulse squares the triangle wave first, giving it a
  faster-attack, snappier shape versus Breath's smooth linear ramp — at a
  quarter period Pulse sits visibly further from its peak than Breath.
- **Rainbow**: full hue sweep, `RAINBOW_PERIOD_MS = 4000`. Rainbow ignores the
  slot's colour entirely (only brightness matters) and returns to its starting
  colour after exactly one period.

### Gamma — why, and why gamma 2

`gamma_channel(v) = v² / 255`, applied per channel by `gamma(c)`.

A WS2812's PWM is linear in duty cycle, but the eye is not: perceived
brightness goes roughly as a fractional power of it. A linear ramp therefore
*looks* like it leaps to nearly full brightness and then plateaus — so a
crossfade between two LEDs, which is how a travelling animation fakes the
positions between them, reads as a snap even when the arithmetic is perfectly
smooth.

The exponent is **2, not the 2.6 a lookup table would normally use**, because
encoding throws away real output: at 2.6 a mid-scale value lands near 46/255
and the whole board looks nearly off. Two is enough to linearise a crossfade,
keeps far more brightness, and — being just `v²/255` — needs no table at all.

**Where to apply it:** once, at the output boundary, immediately before
encoding for the wire. Anywhere earlier and values that get scaled again
afterwards would be double-corrected. (This is also why the perceptual
percentages in `status.rs` are higher than they look — see below.)

Properties the tests pin, and why they matter:

- `gamma_channel(0) == 0` — off must stay off, or an idle board glows.
- Monotonic over the whole range — a table that dips anywhere would make a
  rising ramp visibly stutter, the exact fault gamma exists to remove.
- Full scale stays near full (`gamma_channel(255) > 235`).
- The midpoint lands well below half (between 50 and 90): near 128 the curve
  would be doing nothing, but 2.6-style crushing would darken the board.
- No output step exceeds 3 for a one-unit input step — encoding must not
  amplify a small change into a visible jump.

### Helpers

- `scale(c, brightness)` scales each channel by `brightness/255` (255 leaves
  the colour unchanged, 0 is black).
- `hsv_to_rgb(h, s, v)` is the classic six-region integer HSV→RGB conversion
  (as used by e.g. FastLED / Adafruit_NeoPixel), all channels 0..=255, no
  floating point. `h` wraps every 256 steps; the hue circle is divided into 6
  regions of ~42.67 (`h / 43`), and the remainder is rescaled to roughly
  0..=255 within the region. Expect a few units of rounding residue right at
  region boundaries (e.g. hue 85 is "essentially green": g=255, r≤3, b=0).
- `triangle(t_ms, period_ms)` is a deterministic triangle wave: 0 at phase 0,
  peaking at 255 at half the period, back to 0 at a full period. **The period
  is clamped to a minimum of 2 ms** because a smaller period would make the
  half-period zero and divide by it — and this is not hypothetical: periods
  are derived from a `speed` byte that arrives over the wire, and no animation
  should be able to panic the firmware.
- `lfo_brightness` is the shared Breath/Pulse envelope: it maps the (optionally
  squared, for the sharp/Pulse shape) triangle wave onto
  `min_pct%..100%` of the slot's maximum brightness.

## ring.rs — the 8-LED underglow ring

The ring is the device's status line. Where the per-key LEDs answer "which
agent, and what is it doing", the ring answers the one question you want from
across the desk: *does this need me?*

Every motion has a distinct **shape**, not merely a distinct speed:

| Motion      | Shape                          | Means                        |
|-------------|--------------------------------|------------------------------|
| `Breath`    | whole ring swells together     | calm; nothing is running     |
| `Spin`      | a comet travels round          | something is running         |
| `Alert`     | sharp double-blink             | a decision is waiting on you |
| `Aurora`    | slow cool multi-hue drift      | no host at all               |
| `Searching` | one dim dot orbiting slowly    | host up, daemon down         |

Shape rather than speed because two of these are never on screen at the same
time, so there is nothing to compare a speed against. A swell and a travelling
dot are told apart in one glance with no reference.

Everything is a pure function of `(t_ms, index)`, so it renders identically on
the host and on the device and can be tested without either. `frame` writes
into a caller-supplied slice of any length — the motions scale to it, so an
8-LED ring and a hypothetical 12-LED one both look right, and nothing is
allocated. An empty slice is a no-op, not a panic. Brightness 0 or
`Motion::Off` blanks the ring unconditionally (otherwise "sleep" would not
actually turn the ring off).

### Timing constants and their reasoning

- `SPIN_PERIOD_MS = 1200` — one revolution of the comet at nominal speed;
  `SPIN_TAIL_LEDS = 3` — how far the tail trails behind the head.
- `BREATH_PERIOD_MS = 3400`, `BREATH_MIN_PCT = 25` — the breath dips, it does
  not blink; a breath that reaches zero reads as a blink, which is Alert's job.
- `ALERT_PERIOD_MS = 1100`, `ALERT_FLASH_MS = 90`, `ALERT_GAP_MS = 110` — two
  quick full-brightness flashes, then a long dark rest. The rest between
  double-taps is what makes it read as urgent rather than as a strobe; a test
  pins the duty cycle below 30 %.
- `SEARCH_PERIOD_MS = 4200`, `SEARCH_TAIL_LEDS = 1`, `SEARCH_MAX_PCT = 70` —
  the searching dot is deliberately slow and patient (more than twice Spin's
  period, which is what separates "working on it" from "looking for the
  daemon") and capped at 70 % of the requested brightness, because it is a
  background hint, not a notification. Tests require it to be a single dot and
  markedly quieter than Alert.
- `AURORA_PERIOD_MS = 9000`, `AURORA_MIN_PCT = 50` — Aurora is ambient, so it
  keeps a floor and never goes fully dark (dark would be indistinguishable
  from off).
- **Aurora's hue band: `AURORA_HUE_MIN = 112` to `AURORA_HUE_MAX = 168`.**
  Aurora stays in the cool band, cyan through blue, because warm hues are
  reserved for things that want attention and "no host" does not. The upper
  bound stops at 168 rather than running further into the blues because the
  hue wheel turns violet at 172 — and violet reads as *warm* again, with red
  climbing back above green. A test asserts the rendered colours never go warm
  (blue and green always ≥ red), so the band must not cross that line.
- Aurora **ignores `glow.color` on purpose**: it runs in the state where there
  is no host, so there is no agent whose colour it could be.
- Aurora's drift comes from offsetting each LED's phase by a fraction of the
  period — that per-LED offset is the whole reason it reads as a drift around
  the ring rather than the ring pulsing as one. A second, doubled-rate
  triangle wave modulates brightness to keep it shimmering while it drifts.

### Sub-LED resolution and the comet profile

`SUB = 256`: positions of the travelling motions are tracked in 1/256ths of an
LED so a comet on an 8-LED ring glides instead of stepping between eight
discrete places — at these speeds the stepping is very visible.

`COMET_LEAD_LEDS = 1`: how far *in front of* the head an LED starts to light.
This fixes a real regression: without a leading ramp the comet is **choppy**,
and not subtly so. The head advances in fractions of an LED, but an LED in
front of it used to sit at weight 0 until the head crossed it, at which point
its wrap-around distance snapped from "almost a whole revolution behind" to
zero and it jumped straight to full brightness. The trailing edge faded
smoothly; the leading edge was a hard step, so a revolution read as `count`
discrete jumps rather than a glide. One LED of lead-in is enough: the rise and
the fall then meet exactly at the head, so the brightness profile is
continuous the whole way round. Tests sample at the real firmware frame tick
(16 ms) and assert no single LED's channel ever jumps by more than 90 in one
frame (a full LED step is 255; at ~6 frames per LED a smooth ramp moves ~42
per frame, so 90 catches a snap while leaving headroom for rounding).

`comet_weight` computes an LED's brightness from its wrap-around distance
behind the head: 255 at the head, linear fade over the tail behind it, linear
rise over the lead in front of it, 0 elsewhere. The modulo arithmetic must
wrap correctly across the ring seam — getting it wrong shows up as the tail
vanishing once per revolution, which is easy to miss and awful to look at
(pinned by `comet_weight_wraps_around_the_ring_seam`).

### Speed scaling

`period_for(nominal_ms, speed)` scales a nominal period by `Glow::speed`:
`NOMINAL_SPEED` (128) leaves it unchanged; 255 roughly halves the period;
lower values lengthen it. `speed` is clamped to at least 1 and the result to
at least 1 ms, keeping the callers' modulo arithmetic safe no matter what
arrives over the wire — a zero speed must not divide by zero. Speed changes a
motion's rate without changing its shape (the comet stays the same width).
Brightness is a ceiling for every motion: a dimmer setting can never out-shine
a brighter one at the same instant.

Small helpers: `mul(a, b) = a*b/255` composes two 0..=255 brightness factors;
`envelope(ceiling, wave, min_pct)` maps a 0..=255 wave onto
`min_pct%..100%` of a ceiling.

## status.rs — the visual design, in one place

This module *is* the design. Everything visual is decided here and nowhere
else, so there is a single answer to "what does amber mean" — and so the
answer can be unit-tested rather than eyeballed on hardware.

### The board

```text
      [ 0 ][ 1 ]              agent slots — colour = which agent,
 [ 2 ][ 3 ][ 4 ][ 5 ]                       effect = what it is doing
 [ 6 ][ 7 ][ 8 ][ 9 ]         reserved, dark
 [ ✓ ][ ★ ][ ✗ ]              lit only while a decision is pending
  ◌ ◌ ◌ ◌ ◌ ◌ ◌ ◌             the ring — overall status
```

### Two independent questions

The per-key LEDs and the ring answer different things on purpose:

- **Keys**: *who* is busy, and with what. One key per session, in that agent's
  colour. Six of them, so six sessions are legible at once.
- **Ring**: *does this need me?* One thing, readable from across the room,
  which is why it gets shape-based motions rather than six more colours.

### Why the firmware needs its own copy of this

Two of the link states — `Link::Offline` and `Link::NoDaemon` — exist
precisely when the host cannot tell the device anything. If the device only
ever displayed what it was told, those two would both render as "dark", which
is also what "broken" and "flat battery" look like. So the firmware runs this
module locally and falls back to it whenever the host goes quiet.

### Link state

`Link` is ordered by how much the device knows, least first: `Offline` (no USB
and no BLE host — the device is on its own), `NoDaemon` (a host is there but
nothing is speaking the protocol), `Live` (the daemon is driving the display).

`link_state(host_attached, since_last_frame_ms)` derives the link from two
plain facts. It lives here rather than in the firmware **so it is actually
tested** — a `#[cfg(test)]` block in the firmware crate never runs, because
that crate only ever builds for `xtensa-esp32s3-none-elf`. Rules:

- Frames within `DAEMON_TIMEOUT_MS` mean `Live`, regardless of the USB detect —
  frames arriving over BLE on battery mean USB-detect reads low, but something
  is plainly talking to us, so believe the frames. A few missed heartbeats
  must not flicker the display between modes.
- Past the timeout: `NoDaemon` if a host is attached, else `Offline`.

### Brightness and speed constants

Per-state perceptual percentages: `IDLE_PCT = 62`, `THINKING_PCT = 85`,
`WORKING_PCT = 100`, `AWAITING_PCT = 100`. Idle sits below full because a calm
state should not be the brightest thing on the desk. **These are deliberately
higher than they look**: output is gamma-encoded on the way out
(`gamma_channel`), and a nominal 45 % became barely visible once encoding was
applied — resist the urge to "correct" them downward.

Per-state speeds: `IDLE_SPEED = 96` (slower than nominal),
`THINKING_SPEED = NOMINAL_SPEED` (128), `WORKING_SPEED = 190`. Same motions,
read as different urgencies; a test pins that Working spins faster than
Thinking while sharing its shape.

Other colours and levels:

- "Daemon up, nothing running": a very dim white breath —
  `NO_AGENTS_COLOR = 200, 220, 255` at `NO_AGENTS_PCT = 40` %, at idle speed.
  Present enough to prove the whole chain works, quiet enough to ignore; a
  test pins that it is dimmer than any running agent's ring.
- "Host here, daemon not": **amber**, `NO_DAEMON_COLOR = 255, 150, 0` at
  100 %, with `Motion::Searching`. Amber because it is a problem you can fix,
  as distinct from offline, which is just a fact.
- Offline: `Motion::Aurora` at `OFFLINE_PCT = 85` %. Aurora picks its own
  hues, so `OFFLINE_COLOR = 0, 120, 200` is unused — kept explicit rather
  than leaving a junk value on the wire.
- Tests pin that Offline and NoDaemon differ in both motion *and* colour (so
  neither channel alone has to carry the distinction), and that neither is
  ever dark — dark is indistinguishable from broken, flat, or unplugged.

### Glow builders

- `local_glow(link, brightness)` — the ring for a situation the device can
  work out by itself. Only meaningful for `Offline` and `NoDaemon`; `Live`
  returns `Glow::OFF` because when the daemon is up it is the authority on
  what the ring shows — a local guess would fight the frame the daemon sent.
- `agent_glow(color, state, brightness)` — the ring when a session has focus:
  motion says what is happening, colour says who it is happening to. Idle →
  Breath, Thinking/Working → Spin (different speeds), AwaitingApproval →
  Alert. It takes a colour rather than an `AgentKind` because the user can
  retune the palette (`AgentColors`), and this module should not have to know
  that. Every state renders distinctly; every state keeps the agent's colour.
- `no_agents_glow(brightness)` — the ring when the daemon is up and nothing is
  running.

### Per-key rules

- `slot_effect(state)`: the per-key vocabulary is deliberately quieter than
  the ring's — with up to six keys lit at once, six competing animations
  would be unreadable, so only the state that actually wants a press gets
  movement. Idle → Breath (barely there — the session exists, nothing more),
  Thinking → Breath, Working → Solid, AwaitingApproval → **Pulse**, the one
  per-key animation, so a waiting session is findable among five busy ones.
- `slot_for(color, state, brightness)`: agent colour + state effect + the
  state's percentage of master brightness. Colour identifies *who*; the
  effect carries *what*. A state must never override the colour.
- `action_slot(role, armed, brightness)`: a bottom-row key, `LedSlot::OFF`
  when unarmed. Held `Effect::Solid` rather than pulsing — these are targets
  to hit, and a moving target reads as a warning; the ring's `Motion::Alert`
  is already carrying the urgency, and two things shouting at once is worse
  than one. Their simply *appearing* is the signal that something changed.
- `interrupt_slot(armed, brightness)`: the stop key, at
  `INTERRUPT_PCT = 72` % — kept dimmer than the decision row because it is a
  standing option, not a reply to a question, and should not compete with a
  live prompt for attention.
- `status_slot(color, state, brightness)`: the transparent-keycap status
  light, at `STATUS_PCT = 100` % — it is the board's headline, the one key
  that reads from across the room. It carries the focused agent's colour and
  flashes (Pulse) when that agent wants something; running states are steady,
  so a flash always means "needs you". Nothing focused → off.
- `key_slots(frame)` expands a `LedFrame` into every key's slot, indexed by
  key id. It lives here, rather than on either side, because both ends need
  it: the firmware to paint the chain, and the daemon (and TUI preview) to
  reason about the whole board — two implementations would be two chances for
  the key that lights up and the key that acts to disagree. Note the output
  is *key* order, not LED-chain order; mapping one to the other is
  `layout::LED_FOR_KEY`, and it is the firmware's job.

### Which keys are offered

`action_keys_for(state)` — a lit button that does nothing trains you to stop
believing the lights, so each key appears exactly when pressing it would have
an effect:

- `AwaitingApproval`: approve + deny, but **not** interrupt — nothing is
  executing while the agent waits, so there is nothing to interrupt. (This
  also guarantees stop and deny — which share a hue — never light at the same
  time, which would be genuinely confusing.)
- `Thinking` / `Working`: interrupt only.
- Idle or nothing selected: nothing — no key would do anything.

## startup.rs — the boot animation

Runs once, on power-up, before the agent-state rendering takes over. Beyond
looking good it has two jobs: it **proves the LED chain is alive and correctly
ordered** (a wrong chain index or a swapped colour order is obvious in a
sweep, and invisible in a static colour), and it gives the BLE stack a moment
to come up before the first frame arrives.

Pure and integer-only: every pixel is a function of `(t_ms, index)`, so the
whole animation is unit-testable and needs no clock, no allocation and no
floating point.

- `DURATION_MS = 1400` — long enough to read as deliberate, short enough that
  nobody waits for it.
- Phase 1, the sweep (`SWEEP_MS = 700`): a bright head runs the length of the
  chain, leaving a decaying tail. The head position is scaled so it runs one
  LED past the end (`count + TAIL` over the sweep) and the last pixel gets its
  full moment. `TAIL = 4` LEDs still glow behind the head with linear falloff —
  a longer tail reads as smoother motion, but beyond about a third of the
  chain it just looks like a flash.
- Phase 2, the settle (the remaining 700 ms): the whole chain glows and fades
  linearly to black, handing over to the real frame at black.
- Hues ramp along the chain from `HUE_START = 130` across `HUE_SPAN = 60`
  (cyan through violet, matching the interface's own accent colour). The ramp
  is not just decoration: **a single-colour sweep would hide a wrong chain
  order** — the hue variation along the chain is what makes a misordered
  chain visible.
- `pixel` returns black for `t_ms >= DURATION_MS`, out-of-range indexes and
  empty chains, so a caller that keeps rendering past the end simply sees the
  chain go dark. It must work for both real chain lengths (13 per-key, 8
  underglow) without panicking or leaving anything lit.

## demo.rs — the scripted walk

### Why this exists

The BLE GATT server is not wired yet, so on real hardware the device can only
ever reach `Link::NoDaemon` and `Link::Offline` — the two states it works out
for itself. Everything the *host* drives (agent colours, the spinner, the
decision row) is implemented and unit-tested but has no way to arrive. This
module fabricates those frames locally so the design can be seen on the device
before the link exists.

It is a **demo, not a fallback**: nothing selects it automatically, because a
board inventing agent states it cannot know would be actively misleading. It
lives in this crate rather than the firmware so the script itself is
testable — and because building the frames means going through `status`,
which is the point: if the demo looks right, the real path renders the same
way.

- `SCENE_MS = 4000` — each scene holds long enough to take in a slow Breath
  swell (3.4 s nominal) without the walk becoming tedious.
- `DEMO_BRIGHTNESS = 255` — full scale, because output is gamma-encoded before
  it reaches the LEDs, so anything noticeably below full reads as dim on the
  real device.
- The script is 8 scenes (`SCENE_COUNT = 8`), wrapping: offline; host up but
  daemon down; daemon up with nothing running; Claude thinking; Codex
  working (stop key lit); Grok working; three agents at once focused on
  Claude (the ring follows the focused agent, not an average); Claude
  awaiting approval (ring alerts, approve + deny lit, status flashing). Each
  step carries a label for the serial log.
- A test enforces that the walk covers every motion, every named agent's
  colour, the decision row and the stop key — the demo's whole job is to show
  the design, so anything the design can express must appear somewhere, or it
  ships unverified by eye.
- `scene_at(t_ms)` derives the scene index from a wrapping clock; the index
  must always stay in range whatever `t_ms` is.

## power.rs — the power button

One button, three meanings, told apart by how long it is held:

| Gesture   | Result                              |
|-----------|-------------------------------------|
| short press | wake / turn the lights on         |
| hold ~2 s | power off                           |
| hold ~8 s | reboot into the ROM bootloader      |

The long hold exists as an **escape hatch**: the host can ask the firmware to
enter download mode over BLE, but if the firmware is wedged badly enough that
BLE is not answering, that route is gone. A button hold is handled far enough
down the stack to still work when most of the firmware does not.

It is a pure state machine over `(pressed, now_ms)`, so the timing — the part
that is miserable to debug on a device with no display — is unit-tested here
rather than discovered on hardware.

Constants:

- `HOLD_OFF_MS = 2_000`, `HOLD_BOOTLOADER_MS = 8_000`, `DEBOUNCE_MS = 30`
  (presses shorter than the debounce are contact bounce, not intent).
- A **compile-time** assertion enforces
  `HOLD_BOOTLOADER_MS >= HOLD_OFF_MS * 3`: someone meaning to power the device
  down must not land in the bootloader, so the two gestures are kept far
  apart. It is checked at compile time rather than in a test because it is a
  property of the constants themselves.

Behavioural decisions in `PowerButton::update`:

- **Long holds fire while still held**, which is what makes them feel right:
  the device acts at two seconds rather than when you happen to let go. A
  short press, by contrast, can only be recognised on release — until then it
  is indistinguishable from the start of a hold.
- When the power-off threshold fires, `fired` is deliberately **not** set:
  the press may go on to reach the bootloader threshold, and in the normal
  case the caller powering down is what ends the gesture. Once the bootloader
  threshold fires, `fired` is set so one long press cannot emit repeatedly.
- On release: if a hold already did its thing (or the press lasted past the
  power-off threshold), the release means nothing — releasing after a hold
  must not also wake. Otherwise a press past the debounce is a `Wake`, and
  anything shorter is bounce.
- Held time is computed with `now_ms.wrapping_sub(start)` because embassy's
  millisecond counter wraps; a press across the wrap must not read as a
  multi-hour hold.

## ws2812.rs — SPI encoding of WS2812 data

The Creator Micro 2 drives both of its LED chains from **SPI hosts, not the
RMT peripheral** — that is what the stock firmware does, recovered from its
image (see `docs/hardware/creator-micro-2-pinout-findings.md`). SPI has no
notion of a WS2812 bit, so each one is spelled out as a fixed pattern of SPI
bits whose high time encodes the value:

```text
SPI clock 2.5 MHz  ->  one SPI bit = 400 ns
WS2812 bit         =  3 SPI bits  = 1.2 us

'0' -> 100    400 ns high,  800 ns low
'1' -> 110    800 ns high,  400 ns low
```

This is deliberately the same scheme ESP-IDF's `led_strip` SPI backend uses,
because that is what the vendor firmware drives these exact chains with — **a
known-good configuration on this board beats a merely in-spec one**. Both high
times sit comfortably mid-window (T0H 250–550 ns, T1H 650–950 ns) rather than
near the edges, and three SPI bits per LED bit means each colour byte lands on
exactly three whole SPI bytes.

**Warning: changing the clock without changing the bit patterns silently
produces wrong colours, or nothing at all.** A test
(`the_bit_patterns_land_inside_the_ws2812b_timing_windows`) recomputes T0H,
T1H and the bit period from `SPI_HZ` and asserts they stay inside the WS2812B
windows — it exists specifically to stop someone "optimising" the clock later.

The module lives in this crate, rather than the firmware crate, because it is
pure byte-shuffling and therefore the one part of the LED path that can be
tested on a host. The firmware just hands the resulting buffer to SPI.

Constants and contracts:

- `SPI_HZ = 2_500_000` — the clock this encoding assumes; driving the bus at
  any other rate breaks the timings above.
- `SPI_BITS_PER_BIT = 3`; patterns `PATTERN_ZERO = 0b100`,
  `PATTERN_ONE = 0b110`.
- `BYTES_PER_PIXEL = 9` (24 colour bits × 3 SPI bits = 72 bits).
- `RESET_BYTES = 20`: WS2812 latches when the line is held low for ≥ 50 µs.
  At 2.5 MHz one byte of zeros is 3.2 µs, so 20 bytes is 64 µs — comfortably
  over, and cheap. Every frame (including an empty one) ends with this latch
  gap; a test checks the ≥ 50 µs arithmetic from the constants rather than
  trusting them.
- `buffer_len(pixel_count) = pixel_count * BYTES_PER_PIXEL + RESET_BYTES`.

Encoding details:

- `encode_byte` accumulates the eight 3-bit patterns MSB-first into a 24-bit
  word and then splits it into three bytes — simpler, and less error-prone,
  than trying to place bit triples that straddle byte boundaries by hand. The
  colour MSB is the first bit on the wire.
- **Colour order on the wire is GRB, not RGB** — what the WS2812 family
  expects and what the vendor firmware's `led_strip` default produces. Pure
  red therefore appears in the *second* three-byte group.
- `encode` returns `None` when the buffer is too small rather than writing a
  partial frame — a partial frame would leave the strip un-latched showing
  garbage. Bytes beyond the written length are left alone, so a caller can
  reuse one oversized buffer.

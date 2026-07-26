# What the lights mean

The board has three display surfaces and they answer three different questions.
Keeping them separate is the whole design: when six things are lit at once, a
surface that tries to say two things says neither.

| Surface | Question it answers | Vocabulary |
|---|---|---|
| Agent keys (rows 0–1) | **Who** is busy, and with what? | colour = agent, effect = state |
| Action keys (rows 2–3) | **What can I do right now?** | lit only when a press would act |
| Underglow ring | **Does this need me?** | motion shape |

## The layout

Thirteen keys in rows of **2, 4, 4, 3**.

```
      ┌────┐┌────┐
      │  0 ││  1 │              agent slots 0–1
      └────┘└────┘
┌────┐┌────┐┌────┐┌────┐
│  2 ││  3 ││  4 ││  5 │        agent slots 2–5
└────┘└────┘└────┘└────┘
┌────┐┌────┐┌────┐┌────┐
│STOP││    ││    ││    │        6 = interrupt, 7–9 reserved
└────┘└────┘└────┘└────┘
┌────┐┌────┐┌────┐
│ ✓  ││ ✗  ││ ◉  │             10 approve · 11 deny · 12 status
└────┘└────┘└────┘             12 has the transparent keycap
 ◌ ◌ ◌ ◌ ◌ ◌ ◌ ◌               8-LED underglow ring
```

This is not a guess. Work Louder's shipping firmware embeds its own default
`keymap.json`, and one of the profiles in it is an **agent** profile — the
vendor's own take on this exact use case:

```json
"keymap": [
  ["KV_OAI_AG00",  "KV_OAI_AG01"],
  ["KV_OAI_AG02",  "KV_OAI_AG03",  "KV_OAI_AG04",  "KV_OAI_AG05"],
  ["KV_OAI_ACT06", "KV_OAI_ACT07", "KV_OAI_ACT08", "KV_OAI_ACT09"],
  ["KV_OAI_ACT10", "KV_OAI_ACT11", "KV_OAI_ACT12"]
]
```

Six `AG` keys and seven `ACT` keys, numbered row-major. That six is why
`SLOT_COUNT` is six. Their lighting code splits the same way we do, into
`syncKeysLighting` and `syncAmbientLighting`, and their effect vocabulary
(`solid`, `snake`, `rainbow`, `gradient`, `shallow_breath`) maps almost directly
onto the motions below — `snake` is `Spin`, `shallow_breath` is `Breath`.

Everything above is recovered fact. The one thing the image does **not** state is
`layout::LED_FOR_KEY`, the map from key id to position along the WS2812 chain —
the strip is written as a flat array and the routing is a PCB fact.

**It runs backwards.** Chain position `i` is key `12 - i`; the transparent-keycap
key at the bottom right is chain index 0. The first attempt assumed identity, on
the reasoning that a board numbered row-major is probably wired row-major. It is
not — with identity, approve and deny lit up along the *top* row. Only looking at
the hardware settled it.

To re-check after a board revision: `Device → What the lights are doing →
Identify LEDs` lights one chain position at a time and logs its index.

## Agent colours

Colour says **who**, not what. Three simultaneous Claude sessions are the case
that actually needs telling apart, and colouring by state made them identical.

| Agent | Colour | RGB |
|---|---|---|
| Claude | orange | `255, 120, 30` |
| Codex | white | `230, 230, 230` |
| Grok | purple | `160, 60, 255` |
| anything else | grey | `120, 120, 120` |

White sits below full scale because a saturated WS2812 white swamps a coloured
neighbour. An unrecognised agent stays grey rather than being folded into
Claude — a mystery agent should look like one.

All four are user-overridable (`AgentColors`, TUI → Settings → *Colour for an
agent*), and a test asserts they stay far enough apart in RGB to be
distinguishable.

## Agent state, on the key

Only one state animates. Six competing per-key animations are unreadable; one is
findable.

| State | Effect | Brightness |
|---|---|---|
| Idle | Breath | 62% |
| Thinking | Breath | 85% |
| Working | Solid | 100% |
| Awaiting approval | **Pulse** | 100% |

Those percentages are *perceptual* — output is gamma-encoded afterwards, so they
sit higher than they look. See **Smoothness** below.

## The ring

The ring shows the **focused** session — the one the action keys act on. Motions
differ by *shape*, not speed, because two of them are never on screen together,
so there is nothing to compare a speed against.

| Situation | Motion | Colour |
|---|---|---|
| No host at all | `Aurora` — slow cyan→blue drift | its own hues |
| Host up, daemon down | `Searching` — one dim dot orbiting | amber |
| Daemon up, nothing running | `Breath`, very dim | pale white |
| Agent idle | `Breath`, slow | agent's |
| Agent thinking | `Spin` | agent's |
| Agent working | `Spin`, faster | agent's |
| Agent awaiting approval | `Alert` — sharp double-blink | agent's |

Two details worth stating:

- **Neither disconnected state is dark.** Dark is indistinguishable from broken,
  flat, or unplugged. Offline and daemon-down differ in *both* colour and
  motion, so neither channel alone has to carry it.
- **Aurora ignores the agent colour**, because in that state there is no agent.
  Its hue band stops at 168 rather than running further into the blues: the wheel
  turns violet at 172, and violet reads warm again, which would undo the point of
  a cool ambient state.

## The action keys

They light **only** when a press would do something. A lit button that usually
does nothing teaches you to ignore it, which is precisely wrong for an approval
prompt. Their appearing *is* the notification.

| Key | Colour | Lit when |
|---|---|---|
| 10 Approve | green `0,230,70` | focused session awaits approval |
| 11 Deny | red `255,30,20` | same |
| 12 Status | the focused agent's colour | whenever a session is focused |
| 6 Stop | dim red `180,25,15` | focused session is thinking or working |

Approve and deny are held **solid**, not pulsing: these are targets to hit, and
the ring's `Alert` is already carrying the urgency. Two things shouting at once
is worse than one.

**Key 12 is the transparent keycap**, and it is an indicator rather than a
button — pressing it does nothing. A clear cap passes far more light than a
tinted one, so it is the one key legible from across a room: it carries the
focused agent's own colour, steady while that agent works and flashing when it
wants something. Approve and deny sitting immediately to its left is what makes
them findable without looking — find the lit key, the decisions are next to it.

There is deliberately no "always allow" key. It was on key 12 in an earlier
draft; the status light is a better use of the only transparent cap on the board,
and a third decision key nobody asked for was not worth the space.

Stop sits on its own row, and never lights at the same time as Deny — they share
a hue, so overlap would be genuinely confusing. It is dimmer because it is a
standing option rather than a reply to a question.

## Smoothness

Three independent things made the travelling motions look choppy, and only fixing
all three helped:

1. **The comet only anti-aliased its trailing edge.** An LED in front of the head
   sat at zero until the head crossed it, then snapped to full — so a revolution
   read as eight discrete jumps. `COMET_LEAD_LEDS` adds a one-LED ramp in front,
   so the rise and fall meet at the head and the profile is continuous.
2. **Output was linear.** A WS2812's PWM is linear in duty cycle and the eye is
   not, so a linear crossfade *looks* like a snap. Output is now gamma-encoded
   once, at the boundary, in `gamma_channel`. Gamma **2** rather than the usual
   2.6: at 2.6 a mid-scale value lands near 46/255 and the whole board looks
   nearly off. Two linearises the crossfade and keeps the brightness — and being
   `v²/255`, needs no table.
3. **Frame timing drifted.** The loop used `Timer::after(16ms)` *after* doing the
   work, so the interval was 16 ms plus however long rendering took, and a
   blocking serial write showed up as a hitch. It now uses a `Ticker`, which
   holds phase.

Note that the percentages in `status.rs` are *perceptual*, and were re-tuned
upward once gamma landed — a nominal 45% became barely visible under encoding.

A test samples both travelling motions at the real 16 ms frame rate, across four
speeds, and fails if any LED lurches by more than a smooth ramp would.

## Seeing it without a device link

`Device → What the lights are doing` in the TUI switches the board between:

- **Normal** — real state.
- **Demo** — a scripted walk through all eight scenes, 4 s each.
- **Identify LEDs** — one chain position at a time, logging its index.

This goes down USB-Serial-JTAG as `!` followed by one byte — `!n`, `!d`, `!i`,
`!p`, or `!?` to identify — on the same link the daemon uses. It works on an
already-flashed device with no daemon and no rebuild.

The `!` is not decoration. This console carries the firmware's log output *and*
its command input, and a host tty in its default line discipline echoes whatever
it receives straight back out — so every line the firmware printed arrived back
at the firmware as input. With bare letters as commands that closed a loop:
`link:` contains `i` and `n`, and any line with a `d` in it put the board into
demo mode on its own. That is why a board would appear to enter demo mode by
itself. Anything arriving without the prefix is now dropped silently — replying
would print more output to echo.

For the same reason the host opens the port **raw and non-blocking**. Raw stops
the echo and stops the line discipline rewriting binary frames; non-blocking is
what makes a read timeout mean anything at all. A blocking read on a silent port
never returns, so a deadline checked *between* reads is never reached — that hung
the setup wizard at "Starting the new firmware", waiting for a reply from a ROM
bootloader, which by definition never sends one.

## Where the code lives

The visual decisions are all in **one** place, `openmicro-effects`, which both
the daemon and the firmware depend on:

- `status.rs` — every mapping above. This module *is* the design.
- `ring.rs` — the ring motions, pure integer maths.
- `demo.rs` — a scripted walk through all of it.

That matters because of one asymmetry: **the two disconnected states exist
exactly when the host cannot tell the device anything.** If the device only ever
displayed what it was told, "no daemon" and "no host" would both render as dark.
So the firmware carries the same design and falls back to it locally.

## Why the wire carries meaning, not pixels

`LedFrame` sends *state* — a `Glow { colour, motion, speed }` and three booleans
— and the firmware animates. Sending pixels instead would mean roughly 1.4 kB/s
of BLE writes to say one word, it would stutter on every dropped frame, and it
could not work at all in the case where the daemon is the thing that has gone
away.

The consequence is that silence has to mean something, so the daemon resends the
current frame every `HEARTBEAT_MS` (1.5 s) and the device falls back after
`DAEMON_TIMEOUT_MS` (6 s) — comfortably more than three heartbeats, so a couple
of dropped writes do not make the display flicker between modes.

## Transports

| Transport | Status |
|---|---|
| **Cable** (USB-Serial-JTAG) | **Works.** The default. |
| BLE | Not implemented — `firmware/src/ble.rs` is still a sketch. |
| Mock | Renders into memory, for testing the daemon alone. |

Cable is the default because it is the one that reaches hardware, and it is the
better link anyway: no pairing to lose, no connection to drop mid-session.

Both directions are `wire`-framed (`0xF5 len payload sum`), because the link
shares one byte stream with the firmware's log output. The marker is `0xF5`
specifically because it cannot occur in valid UTF-8, so no log line can be
mistaken for a frame.

Three things about this link were non-obvious enough to be worth recording:

1. **The tty must be put in raw mode.** By default the line discipline *rewrites
   the stream* — `ONLCR` turns 0x0A into CRLF, `IXON` eats 0x11/0x13. ASCII mode
   commands survive that, which is exactly why the first version looked like it
   worked; binary frames containing 0x0A were silently corrupted and the device
   never saw a valid frame.
2. **Drain the RX FIFO before doing any work.** Reading byte-at-a-time with a log
   write in between loses the rest of the USB packet: a six-byte frame arrived as
   its first byte and nothing else. Empty the FIFO first, then process.
3. **A frame's payload bytes must not also be parsed as commands.** The reader
   exposes `in_frame()` for this; without it every payload byte is additionally
   read as a mode command.

## Status

Implemented and unit-tested: all of the above.

Verified on hardware: the boot sweep, `Offline`, `NoDaemon`, the serial
mode-switch round trip, and — via demo mode — every host-driven state.

Verified end-to-end over the cable: the daemon connects, a hook event reaches the
device, and the firmware reports `link=Live` — then falls back to `NoDaemon`
exactly `DAEMON_TIMEOUT_MS` after the daemon stops.

**BLE is still not implemented.** `firmware/src/ble.rs` remains a sketch, so
`Transport::Ble` cannot deliver a frame. Nothing about the rendering depends on
which transport carries it, so wiring the GATT server later changes no display
code.

# TUI style reference

OpenMicro's interface copies the look and feel of `npx skills add`
(<https://github.com/vercel-labs/skills>). This file is the extracted spec, so
the Rust implementation in `crates/openmicro/src/ui/` can be checked against it
without going back to the JavaScript.

Two renderers are in play there, and they do **not** share a palette:

- **@clack/prompts v1.2** — intro, outro, cancel, log, note, select, confirm,
  spinner.
- **A hand-rolled prompt** (`src/prompts/search-multiselect.ts`, 569 lines) —
  the searchable multi-select with the locked section, `❯` cursor, `↓ N more`
  overflow and `Selected: …` summary. This one is not clack.

## Rendering model

No alternate screen. The transcript is append-only in the normal terminal
buffer; only the active prompt block is redrawn in place, and once submitted it
collapses to one or two permanent lines.

Redraw is `ESC[{n}A` (cursor up) + `ESC[J` (erase down) followed by the whole
frame in **one** write. Clearing rows individually makes large prompts flash.
`n` is the number of **wrapped physical rows** of the previous frame, not the
number of logical lines — under-counting leaves ghost rows on narrow terminals.

## Symbols

Unicode is used unless `TERM=linux` (that is the whole fallback condition on
Unix).

| Role | Unicode | Codepoint | ASCII |
| --- | --- | --- | --- |
| step active | `◆` | U+25C6 | `*` |
| step submitted | `◇` | U+25C7 | `o` |
| step cancelled | `■` | U+25A0 | `x` |
| step error | `▲` | U+25B2 | `x` |
| bar start | `┌` | U+250C | `T` |
| bar | `│` | U+2502 | `\|` |
| bar end | `└` | U+2514 | `—` |
| bar horizontal | `─` | U+2500 | `-` |
| radio selected | `●` | U+25CF | `>` |
| radio empty | `○` | U+25CB | ` ` |
| checkbox selected | `◼` | U+25FC | `[+]` |
| checkbox empty | `◻` | U+25FB | `[ ]` |
| info | `●` | U+25CF | `•` |
| success | `◆` | U+25C6 | `*` |
| warn | `▲` | U+25B2 | `!` |
| error | `■` | U+25A0 | `x` |
| note corner top-right | `╮` | U+256E | `+` |
| note connect left | `├` | U+251C | `+` |
| note corner bottom-right | `╯` | U+256F | `+` |

Active and inactive checkboxes are the same glyph; only the colour differs.

Inline glyphs in the search prompt: `❯` U+276F cursor, `▾` U+25BE expanded,
`▸` U+25B8 collapsed, `◐` U+25D0 partially selected, `├─`/`└─` tree branches,
`•` U+2022 locked bullet, `…` U+2026 truncation, `↑↓←→` U+2191/2193/2190/2192.

## Colours

Plain SGR: bold `1`, dim `2`, underline `4`, inverse `7`, strikethrough `9`,
red `31`, green `32`, yellow `33`, blue `34`, magenta `35`, cyan `36`,
gray `90`, bgCyan `46`. Reset partners: `22` for bold/dim, `24`, `27`, `29`,
`39` for colours, `49` for background.

| Element | Colour |
| --- | --- |
| gutter, live clack prompt | cyan |
| gutter, committed lines (intro/outro/log/note) | gray |
| gutter, search prompt (every state) | dim |
| step active `◆` | cyan — but **green** in the search prompt |
| step submitted `◇` | green |
| step cancelled `■` | red |
| step error `▲` | yellow (red when a spinner errors) |
| info `●` | blue |
| selected radio/checkbox | green |
| unselected | dim |
| hovered checkbox | cyan |
| cursor `❯` | cyan |
| partially-selected group `◐` | yellow |
| hints, submitted values | dim |
| cancelled value | strikethrough + dim |
| active row label | underline (not recoloured) |
| spinner frame | magenta |
| `Selected:` label | green |

## Spinner

Frames `◒ ◐ ◓ ◑` (U+25D2, U+25D0, U+25D3, U+25D1) at **80 ms**; ASCII fallback
`• o O 0` at 120 ms. Frame is magenta, followed by **two** spaces then the
message.

Independently of the frame, trailing dots animate: a counter starts at 0 and
increases by 0.125 per tick, wrapping to 0 above 4; the message gets
`".".repeat(floor(counter))` capped at 3. At 80 ms that is one more dot every
640 ms on a 2.56 s cycle.

Trailing ASCII dots are stripped from the message on start; a trailing `…` is
not, which is why the real tool shows `Parsing source….`.

Stop renders `green ◇  msg`, cancel `red ■  msg`, error `red ▲  msg`.

## intro / outro / cancel

```
intro:  gray('┌') + '  ' + msg + '\n'
outro:  gray('│') + '\n' + gray('└') + '  ' + msg + '\n\n'
cancel: gray('└') + '  ' + red(msg) + '\n\n'
```

Two spaces after the corner. Outro and cancel end with a blank line.

## log

One line of bare gutter, then `symbol + '  ' + first line`, with subsequent
lines prefixed by the gutter. An empty line renders as a bare symbol with no
trailing spaces. Variants change only the symbol: info blue `●`, success green
`◆`, step green `◇`, warn yellow `▲`, error red `■`.

## note box

```
│
◇  Title ───────────────────╮
│                           │
│  body line                │
│                           │
├───────────────────────────╯
```

Content is `["", ...wrapped(msg, columns - 6), ""]`; `n` is the display width of
the title and `t = max(widest content line, n) + 2`.

- header: `green('◇') + '  ' + title + ' ' + gray('─'.repeat(max(t - n - 1, 1)) + '╮')`
- body: `gray('│') + '  ' + line + ' '.repeat(t - width(line)) + gray('│')`
- footer: `gray('├' + '─'.repeat(t + 2) + '╯')`

The box's top-left corner *is* the step symbol, and the bottom-left is `├`
rather than `╰`, so the rail continues downward into the next step.

## select / confirm

Frame is `gray('│')`, then `symbol(state) + '  ' + message`, then each option
prefixed with `cyan('│') + '  '`, then the footer:

```
select:      dim('↑/↓') to navigate • dim('Enter:') confirm
multiselect: dim('↑/↓') to navigate • dim('Space:') select • dim('Enter:') confirm
```

joined by `" • "` on a `cyan('│')  ` line, followed by a bare `cyan('└')`.

Clack's window keeps a minimum of 5 rows, scrolls once `cursor >= n - 3`, and
marks overflow with literal dim `...` replacing the first/last option row.

Confirm renders inline: `green('●') Yes dim('/') dim('○') dim('No')`.

## Search multi-select

Layout, top to bottom, in the active state:

```
◆  <bold message>
│
│  ── <bold Title> ── always included ────────────
│    • <bold Label>
│    …and 3 more
│
│  ── <bold Additional agents> ─────────────…
│  Search: <query>█
│  ↑↓ move, space select, enter confirm
│
│ ❯ ● label (hint)
│   ○ label (hint)
│  ↑ 2 more  ↓ 47 more
│
│  Description
│  <wrapped dim detail>
│
│  Selected: A, B, C +10 more
└
```

Rules worth stating because they are easy to over-engineer:

- **Section dashes are fixed counts, not width-filled.** Two leading `─`, then
  the title, then exactly **12** trailing for the locked header and **29** for
  the "Additional" header. No alignment between the two.
- The `Search:` caret is `inverse(' ')`, not a real cursor, and it is always at
  the end — there is no left/right text cursor. The real cursor is never hidden
  in this prompt.
- Overflow is **one** line at the bottom holding both `↑ N more` and `↓ N more`,
  joined by two spaces, all dim.
- The visible window is **cursor-centred**:
  `start = clamp(cursor - maxVisible/2, 0, len - maxVisible)`. This differs from
  clack's own scrolling.
- `Selected:` lists up to **3** labels then ` +N more`. Empty renders as a fully
  dim `Selected: (none)`.
- Filtering is a case-insensitive substring match over label and value. No
  fuzzy matching, no highlight. Any printable key appends and resets the cursor
  to 0; backspace pops and also resets.
- The detail pane is a fixed number of rows so the frame height does not change
  as the cursor moves. Width is `columns - 5`. Greedy word wrap, hard-cut for
  over-long words, `…` appended to the last line when text remains.
- Arrow keys clamp; there is no wraparound. `required` failure is silent —
  Enter simply does nothing.
- Row prefix is `cyan('❯')` on the cursor row and a single space otherwise.

On submit the header becomes a green `◇` and the body collapses to one dim line
of the joined selection, with **no** `└` footer — the gutter continues straight
into the next step. On cancel it becomes a red `■` with a dim, struck-through
`Cancelled`.

## Banner

The `SKILLS` wordmark is a hardcoded 6×43 block of U+2588 `█` plus box-drawing
shadow — figlet's **ANSI Shadow** typeface, pasted rather than generated. It is
printed as a static vertical gray gradient, one 256-colour per row, light at the
top:

```
38;5;250  38;5;248  38;5;245  38;5;243  38;5;240  38;5;238
```

preceded by one blank line. OpenMicro uses its own wordmark in the same
typeface and gradient.

## Animation

The spinner is the only animation: the 4-frame magenta glyph plus the
independent 0–3 trailing dots. No fades, no reveals, no progress bars, no idle
repaint. Every other frame is drawn in response to a keypress, so nothing
outside the spinner needs a tick thread.

The original does not handle `SIGWINCH` in the search prompt, so resizing
mid-prompt corrupts the frame. Ours redraws on resize instead.

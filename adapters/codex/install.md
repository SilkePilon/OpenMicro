# Codex CLI adapter — install

This adapter wires the [Codex CLI](https://github.com/openai/codex) `notify`
mechanism to OpenMicro so your macropad reflects the Codex agent's state.

## How Codex `notify` works

Codex invokes an external program after each turn and appends **one JSON
argument** containing the event. With `notify = ["openmicro-hook",
"codex-notify"]`, Codex runs:

```sh
openmicro-hook codex-notify '{"type":"agent-turn-complete","turn-id":"12345","input-messages":["..."],"last-assistant-message":"..."}'
```

`openmicro-hook codex-notify` reads that payload (first positional arg, falling
back to stdin), maps the event `type` to a state, and pushes
`{agent:"codex", session:<id>, state}` to the running `openmicrod` daemon. It
always exits `0` so it never blocks Codex.

## Event → state mapping

| Codex event `type` (matched, case-insensitive) | OpenMicro state     |
| ---------------------------------------------- | ------------------- |
| contains `approval` / `request` / `notification` | `awaiting_approval` |
| contains `complete` / `finish` / `done` / `idle` / `stop` (e.g. `agent-turn-complete`) | `idle` |
| anything else / unrecognised / non-JSON        | `working`           |

**Confirmed payload (as of this writing):** the documented event is
`type: "agent-turn-complete"`, which fires when a turn finishes and Codex waits
for input — mapped to `idle`. Fields observed: `type`, `turn-id`,
`input-messages`, `last-assistant-message`
(source: <https://github.com/openai/codex> config docs and issues
[#4005](https://github.com/openai/codex/issues/4005),
[#25141](https://github.com/openai/codex/issues/25141)).

**Assumptions (verify against your Codex version):**

- `agent-turn-complete` is currently the only widely-documented `notify` event.
  If your version emits additional types (start / approval / etc.), the
  substring mapping above will classify them; adjust `codex-notify` if your
  version uses different wording.
- The `notify` payload carries **no session id** — only a per-turn `turn-id`.
  The adapter therefore uses the session `"default"` (a per-turn id would spawn a
  new macropad slot every turn). If a future payload adds `session_id`/
  `session-id`, the adapter picks it up automatically.
- Because `agent-turn-complete` is a completion event, the macropad will mainly
  show Codex going `idle` at the end of each turn. Richer live states require a
  finer-grained lifecycle mechanism than `notify` currently exposes.

## 1. Make sure `openmicro-hook` is on `PATH`

```sh
which openmicro-hook
```

If nothing prints, build/install it and add its directory to `PATH` (typically
`~/.cargo/bin` or `target/release`).

## 2. Add the `notify` key to `~/.codex/config.toml`

Copy the line from `config-snippet.toml` into `~/.codex/config.toml`. It must be
a **root** key, placed **before** any `[table]` section:

```toml
notify = ["openmicro-hook", "codex-notify"]

# ... existing [tui], [mcp_servers.*], etc. below ...
```

Validate the file still parses:

```sh
python3 -c 'import tomllib,sys; tomllib.load(open(sys.argv[1],"rb"))' ~/.codex/config.toml
```

## 3. Verify

```sh
openmicrod &     # start the daemon
# Simulate what Codex passes after a turn:
openmicro-hook codex-notify '{"type":"agent-turn-complete","turn-id":"t1"}'
# Read the control socket; codex:default should be idle:
socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/openmicro-ctl.sock" | head -1
# -> {"sessions":[{"agent":"codex","session":"default","state":"idle",...}],...}
```

Then run a real Codex turn and watch the mapped macropad slot go `idle` when the
turn completes. If nothing changes, re-check `openmicro-hook` is on `PATH`, the
`notify` key parses, and `openmicrod` is running.

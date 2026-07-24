# Claude Code adapter — install

This adapter wires Claude Code's lifecycle hooks to OpenMicro so your macropad
reflects the current agent state. Each mapped event runs `openmicro-hook
claude-hook`, which reads the hook event JSON from **stdin**, extracts the
session id, and pushes a state transition to the running `openmicrod` daemon.

## State mapping

| Claude Code event  | OpenMicro state     |
| ------------------ | ------------------- |
| `UserPromptSubmit` | `thinking`          |
| `PreToolUse`       | `working`           |
| `Notification`     | `awaiting_approval` |
| `Stop`             | `idle`              |

## How the session id is obtained (stdin, not an env var)

Claude Code does **not** expose a `$CLAUDE_SESSION_ID` (or any per-session)
environment variable to hook commands. Instead it pipes a JSON object to the
hook command's **stdin**, e.g.:

```json
{ "session_id": "abc123", "hook_event_name": "PreToolUse", ... }
```

`openmicro-hook claude-hook --state <state>` reads that stdin JSON and pulls out
`session_id` itself. If stdin is empty, not JSON, or has no `session_id`, it
falls back to the session `"default"` and still exits `0` — a hook never blocks
the agent. This is why the commands in `hooks.json` no longer reference any
environment variable.

## 1. Make sure `openmicro-hook` is on `PATH`

The hook commands invoke `openmicro-hook` by bare name, so it must be resolvable
from Claude Code's environment.

```sh
which openmicro-hook
```

If nothing prints, install/build it and add its directory to `PATH` (for a Cargo
build that is typically `~/.cargo/bin` or `target/release`). Confirm again with
`which openmicro-hook` before continuing.

## 2. Merge `hooks.json` into `~/.claude/settings.json`

Claude Code reads hook config from `~/.claude/settings.json` (a single JSON
object). Merge the `"hooks"` key from this adapter's `hooks.json` into that file —
do not overwrite the whole file if it already has other settings.

If `~/.claude/settings.json` does not exist yet, you can copy `hooks.json`
directly:

```sh
cp adapters/claude-code/hooks.json ~/.claude/settings.json
```

If it already exists, open it and add the `"hooks"` object from `hooks.json`
alongside your existing keys. If a `"hooks"` key is already present, merge each
event array (`UserPromptSubmit`, `PreToolUse`, `Notification`, `Stop`) rather
than replacing the whole `"hooks"` block. Keep the file valid JSON — validate
with:

```sh
python3 -m json.tool ~/.claude/settings.json
```

## 3. Verify

You can verify the plumbing without Claude Code by feeding a fake hook JSON to
the command exactly the way Claude Code would:

```sh
openmicrod &                                   # start the daemon
echo '{"session_id":"abc","hook_event_name":"PreToolUse"}' \
  | openmicro-hook claude-hook --state working
```

Then read the control socket and confirm the session `claude:abc` is `working`:

```sh
socat - UNIX-CONNECT:"$XDG_RUNTIME_DIR/openmicro-ctl.sock" | head -1
# -> {"sessions":[{"agent":"claude","session":"abc","state":"working",...}],...}
```

End to end with a real session:

1. Start the daemon: `openmicrod` (leave it running).
2. Open a Claude Code session and interact with it.
3. Watch the mapped macropad slot flip as the agent moves through states:
   - submitting a prompt → `thinking`
   - a tool call starts → `working`
   - Claude waits on you (approval / notification) → `awaiting_approval`
   - the turn ends → `idle`

If nothing changes, re-check that `openmicro-hook` is on `PATH` and that
`openmicrod` is running. You can run the command manually (as in the verify
snippet above) to test the daemon path directly.

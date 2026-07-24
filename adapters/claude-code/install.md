# Claude Code adapter — install

This adapter wires Claude Code's lifecycle hooks to OpenMicro so your macropad
reflects the current agent state. Each mapped event runs `openmicro-hook`, which
pushes a state transition to the running `openmicrod` daemon.

## State mapping

| Claude Code event  | OpenMicro state     |
| ------------------ | ------------------- |
| `UserPromptSubmit` | `thinking`          |
| `PreToolUse`       | `working`           |
| `Notification`     | `awaiting_approval` |
| `Stop`             | `idle`              |

## 1. Make sure `openmicro-hook` is on `PATH`

The hook commands invoke `openmicro-hook` by bare name, so it must be resolvable
from Claude Code's environment.

```sh
which openmicro-hook
```

If nothing prints, install/build it and add its directory to `PATH` (for a Cargo
build that is typically `~/.cargo/bin` or `target/release`). Confirm again with
`which openmicro-hook` before continuing.

## 2. Confirm the session-id environment variable

**Important:** the `$CLAUDE_SESSION_ID` used in `hooks.json` is illustrative. The
real environment variable that Claude Code exposes to hook commands may differ.
Before relying on this config, confirm the actual variable name against the
running Claude Code and substitute it everywhere in `hooks.json`.

To discover what is available, temporarily point a hook at a command that dumps
the environment, e.g. replace one command with:

```sh
env | grep -i -E 'session|claude' > /tmp/openmicro-hook-env.txt
```

Trigger the hook (submit a prompt), then inspect `/tmp/openmicro-hook-env.txt`
and use whatever session-id variable Claude Code actually sets. Replace every
`"$CLAUDE_SESSION_ID"` occurrence in `hooks.json` with the confirmed name.

If no per-session variable exists, any stable identifier passed as `--session`
works — the daemon only needs it to be consistent for the lifetime of a session.

## 3. Merge `hooks.json` into `~/.claude/settings.json`

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

## 4. Verify

1. Start the daemon: `openmicrod` (leave it running).
2. Open a Claude Code session and interact with it.
3. Watch the mapped macropad slot flip as the agent moves through states:
   - submitting a prompt → `thinking`
   - a tool call starts → `working`
   - Claude waits on you (approval / notification) → `awaiting_approval`
   - the turn ends → `idle`

If nothing changes, re-check that `openmicro-hook` is on `PATH`, that the
session-id variable is correct (step 2), and that `openmicrod` is running. You
can run the hook command manually to test the daemon path:

```sh
openmicro-hook --agent claude --session test-session --state working
```

// Installed by OpenMicro (openmicro > Coding agents). Safe to delete.
const AGENT = "opencode"

function push(session, state) {
  try {
    Bun.spawn(
      ["openmicro-hook", "push", "--agent", AGENT, "--session", session || "default", "--state", state],
      { stdin: "ignore", stdout: "ignore", stderr: "ignore" },
    ).unref()
  } catch (_) {}
}

export const OpenMicro = async () => ({
  "chat.message": async ({ sessionID }) => push(sessionID, "thinking"),
  "tool.execute.before": async ({ sessionID }) => push(sessionID, "working"),
  "permission.ask": async (permission) => push(permission.sessionID, "awaiting_approval"),
  event: async ({ event }) => {
    if (event.type === "session.idle") push(event.properties.sessionID, "idle")
    else if (event.type === "permission.replied") push(event.properties.sessionID, "working")
  },
})

use std::io::Write;
use std::os::unix::net::UnixStream;

use clap::Parser;

#[derive(Parser)]
#[command(about = "Push an agent state event to openmicrod")]
struct Args {
    #[arg(long)]
    agent: String,
    #[arg(long)]
    session: String,
    #[arg(long)]
    state: String,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let rt = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".into());
    let path = format!("{rt}/openmicro.sock");
    let payload = serde_json::json!({
        "agent": args.agent,
        "session": args.session,
        "state": args.state,
    });
    // Best-effort: if the daemon is down, exit 0 silently so hooks never block agents.
    if let Ok(mut stream) = UnixStream::connect(&path) {
        let mut line = payload.to_string();
        line.push('\n');
        let _ = stream.write_all(line.as_bytes());
    }
    Ok(())
}

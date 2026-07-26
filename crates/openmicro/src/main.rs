//! `openmicro` — the whole user interface, in one command.
//!
//! There are no subcommands. Running the binary opens a menu (see [`app`]) from
//! which every action is reachable: setting the device up, flashing firmware,
//! wiring coding agents, controlling the background service, and removing the
//! lot again.

mod agents;
mod app;
mod client;
mod daemon;
mod display;
mod firmware;
mod flash;
mod probe;
mod prompt;
mod uninstall;
mod wldevice;

fn main() -> anyhow::Result<()> {
    app::run()
}

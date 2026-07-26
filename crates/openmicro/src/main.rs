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

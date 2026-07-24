mod config;
mod control;
mod device;
mod engine;
mod focus;
mod ingress;
mod render;
mod session;

use std::sync::Arc;
use tokio::sync::Mutex;

use device::MockDevice;
use engine::Engine;

fn runtime_dir() -> std::path::PathBuf {
    std::env::var("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir())
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = config::load();
    let engine = Arc::new(Mutex::new(Engine::new(cfg.brightness)));
    let device: Arc<Mutex<dyn device::DeviceLink + Send>> =
        Arc::new(Mutex::new(MockDevice::new()));

    let rt = runtime_dir();
    let hook_path = rt.join("openmicro.sock");
    let ctl_path = rt.join("openmicro-ctl.sock");

    let ingress = tokio::spawn(ingress::serve(hook_path, engine.clone(), device.clone()));
    let control = tokio::spawn(control::serve(ctl_path, engine.clone()));

    println!("openmicrod running (mock device). Ctrl-C to stop.");
    tokio::select! {
        r = ingress => { r??; }
        r = control => { r??; }
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

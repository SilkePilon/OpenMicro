mod action;
mod ble;
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

use config::Transport;
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

    // Channel for device->host input events. In P1 the daemon just drains it;
    // P2 will route these into the engine.
    let (input_tx, mut input_rx) =
        tokio::sync::mpsc::unbounded_channel::<openmicro_proto::InputEvent>();

    let device: Arc<Mutex<dyn device::DeviceLink + Send>> = match cfg.transport {
        Transport::Mock => {
            drop(input_tx);
            Arc::new(Mutex::new(MockDevice::new()))
        }
        Transport::Ble => match ble::BleDevice::connect(input_tx).await {
            Ok(dev) => {
                println!("openmicrod: BLE device connected.");
                Arc::new(Mutex::new(dev))
            }
            Err(e) => {
                eprintln!("openmicrod: BLE connect failed ({e}); falling back to mock device.");
                Arc::new(Mutex::new(MockDevice::new()))
            }
        },
    };

    // Drain input events (P2 will route them into the engine).
    tokio::spawn(async move {
        while let Some(ev) = input_rx.recv().await {
            eprintln!("openmicrod: input event (unrouted): {ev:?}");
        }
    });

    let rt = runtime_dir();
    let hook_path = rt.join("openmicro.sock");
    let ctl_path = rt.join("openmicro-ctl.sock");

    let ingress = tokio::spawn(ingress::serve(hook_path, engine.clone(), device.clone()));
    let control = tokio::spawn(control::serve(ctl_path, engine.clone()));

    println!("openmicrod running (transport: {:?}). Ctrl-C to stop.", cfg.transport);
    tokio::select! {
        r = ingress => { r??; }
        r = control => { r??; }
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

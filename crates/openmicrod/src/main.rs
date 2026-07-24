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
mod sleeper;

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
    let mut engine_init = Engine::new(cfg.brightness);
    engine_init.colors = cfg.colors;
    engine_init.sleep_minutes = cfg.sleep_minutes;
    let engine = Arc::new(Mutex::new(engine_init));

    // Shared last-activity clock: touched by every processed hook event and
    // physical input; read by the idle-sleep timer.
    let clock = sleeper::ActivityClock::new();

    // Channel for device->host input events. In P1 the daemon just drains it;
    // P2 will route these into the engine.
    let (input_tx, mut input_rx) =
        tokio::sync::mpsc::unbounded_channel::<openmicro_proto::InputEvent>();
    // Channel for device->host battery readings, drained into `battery` below.
    let (battery_tx, mut battery_rx) =
        tokio::sync::mpsc::unbounded_channel::<openmicro_proto::Battery>();

    // Latest known battery reading; None on the mock transport / before first read.
    let battery: Arc<Mutex<Option<openmicro_proto::Battery>>> = Arc::new(Mutex::new(None));

    let device: Arc<Mutex<dyn device::DeviceLink + Send>> = match cfg.transport {
        Transport::Mock => {
            drop(input_tx);
            drop(battery_tx);
            Arc::new(Mutex::new(MockDevice::new()))
        }
        Transport::Ble => match ble::BleDevice::connect(input_tx, battery_tx).await {
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

    // Drain battery readings into the shared latest-value cell.
    let battery_store = battery.clone();
    tokio::spawn(async move {
        while let Some(b) = battery_rx.recv().await {
            *battery_store.lock().await = Some(b);
        }
    });

    // Route device input events into the engine. To respect the engine->device
    // lock order used by `ingress` (and to avoid holding the engine lock across
    // routing), we snapshot the slot->session map under a short engine lock,
    // drop it, route purely, then re-lock engine+device to apply.
    let engine_in = engine.clone();
    let device_in = device.clone();
    let clock_in = clock.clone();
    tokio::spawn(async move {
        while let Some(ev) = input_rx.recv().await {
            clock_in.touch();
            let maybe_action = {
                let slot_map: Vec<_> = {
                    let eng = engine_in.lock().await;
                    let lookup = eng.slot_lookup();
                    (0..openmicro_proto::SLOT_COUNT).map(lookup).collect()
                };
                let lookup = |i: usize| slot_map.get(i).cloned().flatten();
                let view = action::RouterView { slot_session: &lookup };
                action::route(&ev, &view)
            };
            if let Some(act) = maybe_action {
                let mut eng = engine_in.lock().await;
                let mut dev = device_in.lock().await;
                eng.apply_action(act, &mut *dev).await;
            }
        }
    });

    let rt = runtime_dir();
    let hook_path = rt.join("openmicro.sock");
    let ctl_path = rt.join("openmicro-ctl.sock");

    let ingress =
        tokio::spawn(ingress::serve(hook_path, engine.clone(), device.clone(), clock.clone()));
    let control =
        tokio::spawn(control::serve(ctl_path, engine.clone(), device.clone(), battery.clone()));
    tokio::spawn(sleeper::serve(clock.clone(), engine.clone(), device.clone()));

    println!("openmicrod running (transport: {:?}). Ctrl-C to stop.", cfg.transport);
    tokio::select! {
        r = ingress => { r??; }
        r = control => { r??; }
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}

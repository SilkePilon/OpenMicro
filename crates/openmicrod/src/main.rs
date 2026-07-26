mod action;
mod ble;
mod cable;
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
use tokio::task::JoinSet;

use config::Transport;
use device::MockDevice;
use engine::Engine;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cfg = config::load();
    let mut engine_init = Engine::new(cfg.brightness);
    engine_init.colors = cfg.colors;
    engine_init.sleep_minutes = cfg.sleep_minutes.min(engine::MAX_SLEEP_MINUTES);
    let engine = Arc::new(Mutex::new(engine_init));

    let clock = sleeper::ActivityClock::new();

    let (input_tx, mut input_rx) =
        tokio::sync::mpsc::unbounded_channel::<openmicro_proto::InputEvent>();
    let (battery_tx, mut battery_rx) =
        tokio::sync::mpsc::unbounded_channel::<openmicro_proto::Battery>();

    let battery: Arc<Mutex<Option<openmicro_proto::Battery>>> = Arc::new(Mutex::new(None));

    let device: Arc<Mutex<dyn device::DeviceLink + Send>> = match cfg.transport {
        Transport::Mock => {
            drop(input_tx);
            drop(battery_tx);
            Arc::new(Mutex::new(MockDevice::new()))
        }
        Transport::Cable => {
            drop(battery_tx);
            match cable::CableDevice::open(input_tx) {
                Ok(dev) => {
                    println!("openmicrod: device connected over the cable ({}).", dev.port().display());
                    Arc::new(Mutex::new(dev))
                }
                Err(e) => {
                    eprintln!(
                        "openmicrod: cable connect failed ({e}); falling back to mock device."
                    );
                    Arc::new(Mutex::new(MockDevice::new()))
                }
            }
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

    let mut tasks: JoinSet<(&'static str, anyhow::Result<()>)> = JoinSet::new();

    let battery_store = battery.clone();
    tasks.spawn(async move {
        while let Some(b) = battery_rx.recv().await {
            *battery_store.lock().await = Some(b);
        }
        ("battery-drain", Ok(()))
    });

    let engine_in = engine.clone();
    let device_in = device.clone();
    let clock_in = clock.clone();
    tasks.spawn(async move {
        while let Some(ev) = input_rx.recv().await {
            clock_in.touch();
            let maybe_action = {
                let (slot_map, focus): (Vec<_>, _) = {
                    let eng = engine_in.lock().await;
                    let lookup = eng.slot_lookup();
                    let slots = (0..openmicro_proto::SLOT_COUNT).map(lookup).collect();
                    (slots, eng.focused())
                };
                let lookup = |i: usize| slot_map.get(i).cloned().flatten();
                let view = action::RouterView { slot_session: &lookup, focus };
                action::route(&ev, &view)
            };
            if let Some(act) = maybe_action {
                let mut eng = engine_in.lock().await;
                let mut dev = device_in.lock().await;
                eng.apply_action(act, &mut *dev).await;
            }
        }
        ("input-routing", Ok(()))
    });

    let engine_hb = engine.clone();
    let device_hb = device.clone();
    tasks.spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_millis(
            openmicro_proto::HEARTBEAT_MS,
        ));
        loop {
            tick.tick().await;
            let eng = engine_hb.lock().await;
            let mut dev = device_hb.lock().await;
            eng.heartbeat(&mut *dev).await;
        }
    });

    let hook_path = openmicro_proto::paths::hook_socket();
    let ctl_path = openmicro_proto::paths::control_socket();

    {
        let engine = engine.clone();
        let device = device.clone();
        let clock = clock.clone();
        tasks.spawn(async move { ("ingress", ingress::serve(hook_path, engine, device, clock).await) });
    }
    {
        let engine = engine.clone();
        let device = device.clone();
        let battery = battery.clone();
        tasks.spawn(async move { ("control", control::serve(ctl_path, engine, device, battery).await) });
    }
    {
        let engine = engine.clone();
        let device = device.clone();
        let clock = clock.clone();
        tasks.spawn(async move {
            sleeper::serve(clock, engine, device).await;
            ("sleeper", Ok(()))
        });
    }

    fn is_fatal_task(name: &str) -> bool {
        matches!(name, "ingress" | "control")
    }

    println!("openmicrod running (transport: {:?}). Ctrl-C to stop.", cfg.transport);
    loop {
        tokio::select! {
            res = tasks.join_next() => {
                match res {
                    Some(Ok((name, Ok(())))) => {
                        eprintln!("openmicrod: task '{name}' exited");
                        if is_fatal_task(name) {
                            eprintln!("openmicrod: '{name}' is a core task; shutting down.");
                            anyhow::bail!("core task '{name}' exited unexpectedly");
                        }
                    }
                    Some(Ok((name, Err(e)))) => {
                        eprintln!("openmicrod: task '{name}' exited with error: {e}");
                        if is_fatal_task(name) {
                            eprintln!("openmicrod: '{name}' is a core task; shutting down.");
                            return Err(e.context(format!("core task '{name}' failed")));
                        }
                    }
                    Some(Err(join_err)) => {
                        eprintln!("openmicrod: a background task panicked: {join_err}");
                        return Err(anyhow::Error::from(join_err).context("background task panicked"));
                    }
                    None => {
                        eprintln!("openmicrod: all background tasks have exited; shutting down.");
                        anyhow::bail!("all background tasks exited");
                    }
                }
            }
            _ = tokio::signal::ctrl_c() => {
                eprintln!("openmicrod: received Ctrl-C, shutting down.");
                return Ok(());
            }
        }
    }
}

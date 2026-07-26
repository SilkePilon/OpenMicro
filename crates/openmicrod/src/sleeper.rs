use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::sync::Mutex as AsyncMutex;
use tokio::time::interval;

use crate::device::DeviceLink;
use crate::engine::Engine;

const TICK: Duration = Duration::from_secs(15);

#[derive(Clone)]
pub struct ActivityClock(Arc<Mutex<Instant>>);

impl ActivityClock {
    pub fn new() -> Self {
        ActivityClock(Arc::new(Mutex::new(Instant::now())))
    }

    pub fn touch(&self) {
        *self.0.lock().unwrap() = Instant::now();
    }

    pub fn idle(&self) -> Duration {
        self.0.lock().unwrap().elapsed()
    }
}

impl Default for ActivityClock {
    fn default() -> Self {
        Self::new()
    }
}

pub fn should_sleep(idle: Duration, sleep_minutes: u32, already_asleep: bool) -> bool {
    if already_asleep || sleep_minutes == 0 {
        return false;
    }
    idle >= Duration::from_secs(sleep_minutes as u64 * 60)
}

pub async fn serve(
    clock: ActivityClock,
    engine: Arc<AsyncMutex<Engine>>,
    device: Arc<AsyncMutex<dyn DeviceLink + Send>>,
) {
    let mut tick = interval(TICK);
    loop {
        tick.tick().await;
        let (sleep_minutes, asleep) = {
            let eng = engine.lock().await;
            (eng.sleep_minutes, eng.asleep)
        };
        if should_sleep(clock.idle(), sleep_minutes, asleep) {
            let mut eng = engine.lock().await;
            let mut dev = device.lock().await;
            eng.sleep(&mut *dev).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_when_sleep_minutes_zero() {
        assert!(!should_sleep(Duration::from_secs(9999), 0, false));
    }

    #[test]
    fn not_when_already_asleep() {
        assert!(!should_sleep(Duration::from_secs(9999), 3, true));
    }

    #[test]
    fn sleeps_once_idle_reaches_threshold() {
        assert!(!should_sleep(Duration::from_secs(179), 3, false));
        assert!(should_sleep(Duration::from_secs(180), 3, false));
        assert!(should_sleep(Duration::from_secs(600), 3, false));
    }

    #[test]
    fn clock_touch_resets_idle() {
        let clock = ActivityClock::new();
        clock.touch();
        assert!(clock.idle() < Duration::from_secs(1));
    }
}

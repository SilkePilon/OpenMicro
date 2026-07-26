use std::time::{Duration, Instant};

use async_trait::async_trait;
use bluer::gatt::remote::{Characteristic, CharacteristicWriteRequest};
use bluer::gatt::WriteOp;
use bluer::{Adapter, AdapterEvent, Session};
use futures::StreamExt;
use openmicro_proto::ble::{
    ADV_NAME_PREFIX, BATTERY_LEVEL_UUID, BATTERY_SERVICE_UUID, INPUT_CHAR_UUID, LED_CHAR_UUID,
    OPENMICRO_SERVICE_UUID,
};
use openmicro_proto::{Battery, InputEvent, LedFrame};
use tokio::sync::mpsc::UnboundedSender;
use uuid::Uuid;

fn bt_uuid16(v: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb_u128 | ((v as u128) << 96))
}

use crate::device::DeviceLink;

const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
const BACKOFF_CAP_SECS: u64 = 30;
const RECONNECT_COOLDOWN: Duration = Duration::from_secs(5);
const RECONNECT_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(2);

pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 2u64.checked_pow(attempt).unwrap_or(u64::MAX).min(BACKOFF_CAP_SECS);
    Duration::from_secs(secs)
}

struct Handles {
    led: Characteristic,
    input: Characteristic,
    battery: Option<Characteristic>,
}

pub struct BleDevice {
    adapter: Adapter,
    led: Characteristic,
    last: LedFrame,
    connected: bool,
    last_reconnect_attempt: Option<Instant>,
    input_tx: UnboundedSender<InputEvent>,
    battery_tx: UnboundedSender<Battery>,
    input_task: tokio::task::JoinHandle<()>,
    battery_task: Option<tokio::task::JoinHandle<()>>,
}

impl BleDevice {
    fn reconnect_cooldown_elapsed(&self) -> bool {
        match self.last_reconnect_attempt {
            None => true,
            Some(t) => t.elapsed() >= RECONNECT_COOLDOWN,
        }
    }
}

impl BleDevice {
    pub async fn connect(
        input_tx: UnboundedSender<InputEvent>,
        battery_tx: UnboundedSender<Battery>,
    ) -> anyhow::Result<BleDevice> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;

        let handles = discover_and_resolve(&adapter).await?;
        let input_task = spawn_input_task(&handles.input, input_tx.clone()).await?;
        let battery_task = match &handles.battery {
            Some(battery) => Some(spawn_battery_task(battery, battery_tx.clone()).await?),
            None => {
                eprintln!("ble: device exposes no Battery Service; battery unavailable");
                None
            }
        };

        Ok(BleDevice {
            adapter,
            led: handles.led,
            last: LedFrame::BLANK,
            connected: true,
            last_reconnect_attempt: None,
            input_tx,
            battery_tx,
            input_task,
            battery_task,
        })
    }

    pub async fn reconnect(&mut self, max_attempts: u32) -> anyhow::Result<()> {
        for attempt in 0..max_attempts {
            match discover_and_resolve(&self.adapter).await {
                Ok(handles) => {
                    self.input_task.abort();
                    if let Some(task) = self.battery_task.take() {
                        task.abort();
                    }
                    self.input_task = spawn_input_task(&handles.input, self.input_tx.clone()).await?;
                    self.battery_task = match &handles.battery {
                        Some(battery) => {
                            Some(spawn_battery_task(battery, self.battery_tx.clone()).await?)
                        }
                        None => None,
                    };
                    self.led = handles.led;
                    self.connected = true;
                    return Ok(());
                }
                Err(e) => {
                    eprintln!("ble: reconnect attempt {attempt} failed: {e}");
                    tokio::time::sleep(backoff_delay(attempt)).await;
                }
            }
        }
        anyhow::bail!("ble: reconnect exhausted after {max_attempts} attempts")
    }
}

impl Drop for BleDevice {
    fn drop(&mut self) {
        self.input_task.abort();
        if let Some(task) = self.battery_task.take() {
            task.abort();
        }
    }
}

async fn discover_and_resolve(adapter: &Adapter) -> anyhow::Result<Handles> {
    let service_uuid = Uuid::from_u128(OPENMICRO_SERVICE_UUID);
    let device = tokio::time::timeout(DISCOVERY_TIMEOUT, find_device(adapter, service_uuid))
        .await
        .map_err(|_| anyhow::anyhow!("ble: discovery timed out after {DISCOVERY_TIMEOUT:?}"))??;

    if !device.is_connected().await? {
        device.connect().await?;
    }

    let led_uuid = Uuid::from_u128(LED_CHAR_UUID);
    let input_uuid = Uuid::from_u128(INPUT_CHAR_UUID);
    let battery_service_uuid = bt_uuid16(BATTERY_SERVICE_UUID);
    let battery_level_uuid = bt_uuid16(BATTERY_LEVEL_UUID);
    let mut led = None;
    let mut input = None;
    let mut battery = None;

    for service in device.services().await? {
        let svc_uuid = service.uuid().await?;
        if svc_uuid == service_uuid {
            for ch in service.characteristics().await? {
                let uuid = ch.uuid().await?;
                if uuid == led_uuid {
                    led = Some(ch);
                } else if uuid == input_uuid {
                    input = Some(ch);
                }
            }
        } else if svc_uuid == battery_service_uuid {
            for ch in service.characteristics().await? {
                if ch.uuid().await? == battery_level_uuid {
                    battery = Some(ch);
                }
            }
        }
    }

    match (led, input) {
        (Some(led), Some(input)) => Ok(Handles { led, input, battery }),
        _ => anyhow::bail!("ble: OpenMicro GATT characteristics not found on device"),
    }
}

async fn find_device(adapter: &Adapter, service_uuid: Uuid) -> anyhow::Result<bluer::Device> {
    let mut events = adapter.discover_devices().await?;
    while let Some(event) = events.next().await {
        let addr = match event {
            AdapterEvent::DeviceAdded(addr) => addr,
            _ => continue,
        };
        let device = adapter.device(addr)?;
        if device_matches(&device, service_uuid).await {
            return Ok(device);
        }
    }
    anyhow::bail!("ble: device discovery stream ended without a match")
}

async fn device_matches(device: &bluer::Device, service_uuid: Uuid) -> bool {
    if let Ok(Some(uuids)) = device.uuids().await {
        if uuids.contains(&service_uuid) {
            return true;
        }
    }
    if let Ok(Some(name)) = device.name().await {
        if name.starts_with(ADV_NAME_PREFIX) {
            return true;
        }
    }
    false
}

async fn spawn_input_task(
    input: &Characteristic,
    input_tx: UnboundedSender<InputEvent>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    let mut notify = Box::pin(input.notify().await?);
    Ok(tokio::spawn(async move {
        while let Some(bytes) = notify.next().await {
            match InputEvent::decode(&bytes) {
                Ok(ev) => {
                    if input_tx.send(ev).is_err() {
                        break;
                    }
                }
                Err(e) => eprintln!("ble: failed to decode InputEvent: {e}"),
            }
        }
    }))
}

async fn spawn_battery_task(
    battery: &Characteristic,
    battery_tx: UnboundedSender<Battery>,
) -> anyhow::Result<tokio::task::JoinHandle<()>> {
    if let Ok(bytes) = battery.read().await {
        if let Some(&pct) = bytes.first() {
            let _ = battery_tx.send(Battery { pct, charging: false });
        }
    }
    let mut notify = Box::pin(battery.notify().await?);
    Ok(tokio::spawn(async move {
        while let Some(bytes) = notify.next().await {
            match bytes.first() {
                Some(&pct) => {
                    if battery_tx.send(Battery { pct, charging: false }).is_err() {
                        break;
                    }
                }
                None => eprintln!("ble: empty battery-level notification"),
            }
        }
    }))
}

#[async_trait]
impl DeviceLink for BleDevice {
    async fn set_leds(&mut self, frame: &LedFrame) {
        self.last = *frame;
        let bytes = match frame.encode() {
            Ok(b) => b,
            Err(e) => {
                eprintln!("ble: failed to encode LedFrame: {e}");
                return;
            }
        };

        if !self.connected && !self.reconnect_cooldown_elapsed() {
            return;
        }

        let req = CharacteristicWriteRequest {
            op_type: WriteOp::Command,
            ..Default::default()
        };
        if let Err(e) = self.led.write_ext(&bytes, &req).await {
            eprintln!("ble: LED write failed, marking disconnected: {e}");
            let was_already_down = !self.connected;
            self.connected = false;
            self.last_reconnect_attempt = Some(Instant::now());

            if !was_already_down {
                return;
            }

            match tokio::time::timeout(RECONNECT_ATTEMPT_TIMEOUT, self.reconnect(1)).await {
                Ok(Ok(())) => {
                    if let Err(e) = self.led.write_ext(&bytes, &req).await {
                        eprintln!("ble: LED write failed again after reconnect: {e}");
                        self.connected = false;
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("ble: reconnect failed: {e}");
                    self.connected = false;
                }
                Err(_) => {
                    eprintln!(
                        "ble: reconnect attempt timed out after {RECONNECT_ATTEMPT_TIMEOUT:?}; will retry after cooldown"
                    );
                    self.connected = false;
                }
            }
        }
    }

    fn last_frame(&self) -> LedFrame {
        self.last
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_is_capped_and_exponential() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(3), Duration::from_secs(8));
        assert_eq!(backoff_delay(4), Duration::from_secs(16));
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(10), Duration::from_secs(30));
        assert_eq!(backoff_delay(1000), Duration::from_secs(30));
    }

    #[test]
    fn bt_uuid16_expands_to_base_uuid() {
        assert_eq!(
            bt_uuid16(0x180F),
            Uuid::parse_str("0000180f-0000-1000-8000-00805f9b34fb").unwrap()
        );
        assert_eq!(
            bt_uuid16(0x2A19),
            Uuid::parse_str("00002a19-0000-1000-8000-00805f9b34fb").unwrap()
        );
    }
}

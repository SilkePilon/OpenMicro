//! Real Bluetooth Low Energy `DeviceLink` backend built on `bluer` (BlueZ).
//!
//! Talks to (future) OpenMicro firmware over the custom GATT service defined in
//! `openmicro_proto::ble`. The daemon keeps `MockDevice` as the default; this
//! backend is only used when the config selects `Transport::Ble`.

use std::time::Duration;

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

/// Expand a 16-bit Bluetooth SIG UUID to its full 128-bit form using the
/// Bluetooth Base UUID (`0000xxxx-0000-1000-8000-00805f9b34fb`).
fn bt_uuid16(v: u16) -> Uuid {
    Uuid::from_u128(0x0000_0000_0000_1000_8000_0080_5f9b_34fb_u128 | ((v as u128) << 96))
}

use crate::device::DeviceLink;

/// How long a single discovery scan is allowed to run before giving up.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(15);
/// Backoff is capped at this many seconds.
const BACKOFF_CAP_SECS: u64 = 30;

/// Capped exponential backoff: `min(2^attempt, 30)` seconds.
///
/// Pure function so it can be unit-tested without a Bluetooth adapter.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 2u64.checked_pow(attempt).unwrap_or(u64::MAX).min(BACKOFF_CAP_SECS);
    Duration::from_secs(secs)
}

/// A connected set of GATT characteristics. The battery level characteristic
/// is optional: not every device exposes the standard Battery Service.
struct Handles {
    led: Characteristic,
    input: Characteristic,
    battery: Option<Characteristic>,
}

/// BLE-backed device link.
pub struct BleDevice {
    adapter: Adapter,
    led: Characteristic,
    last: LedFrame,
    connected: bool,
    input_tx: UnboundedSender<InputEvent>,
}

impl BleDevice {
    /// Discover an OpenMicro device, connect, and resolve GATT handles.
    ///
    /// The caller owns the receiving half of `input_tx`; input notifications are
    /// decoded on a background task and forwarded through the channel.
    pub async fn connect(
        input_tx: UnboundedSender<InputEvent>,
        battery_tx: UnboundedSender<Battery>,
    ) -> anyhow::Result<BleDevice> {
        let session = Session::new().await?;
        let adapter = session.default_adapter().await?;
        adapter.set_powered(true).await?;

        let handles = discover_and_resolve(&adapter).await?;
        spawn_input_task(&handles.input, input_tx.clone()).await?;
        if let Some(battery) = &handles.battery {
            spawn_battery_task(battery, battery_tx).await?;
        } else {
            eprintln!("ble: device exposes no Battery Service; battery unavailable");
        }

        Ok(BleDevice {
            adapter,
            led: handles.led,
            last: LedFrame::BLANK,
            connected: true,
            input_tx,
        })
    }

    /// Re-run discovery and re-resolve handles using capped exponential backoff.
    ///
    /// Wired for P2 (input routing / resilience); kept off the hot render path.
    #[allow(dead_code)]
    pub async fn reconnect(&mut self, max_attempts: u32) -> anyhow::Result<()> {
        for attempt in 0..max_attempts {
            match discover_and_resolve(&self.adapter).await {
                Ok(handles) => {
                    spawn_input_task(&handles.input, self.input_tx.clone()).await?;
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

/// Scan for a device advertising our service (or whose name starts with the
/// advertised prefix), connect, and locate the LED + INPUT characteristics.
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

/// Drive discovery until a matching device is found.
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

/// A device matches if it advertises our service UUID or its name has our prefix.
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

/// Subscribe to the INPUT characteristic and forward decoded events.
async fn spawn_input_task(
    input: &Characteristic,
    input_tx: UnboundedSender<InputEvent>,
) -> anyhow::Result<()> {
    let mut notify = Box::pin(input.notify().await?);
    tokio::spawn(async move {
        while let Some(bytes) = notify.next().await {
            match InputEvent::decode(&bytes) {
                Ok(ev) => {
                    if input_tx.send(ev).is_err() {
                        break; // receiver dropped
                    }
                }
                Err(e) => eprintln!("ble: failed to decode InputEvent: {e}"),
            }
        }
    });
    Ok(())
}

/// Read the initial battery level and subscribe to its notifications,
/// forwarding each reading as a `Battery`.
///
/// The standard Battery Level characteristic (0x2A19) is a single byte, the
/// percentage 0..=100. Plain BLE Battery Service carries no charging state, so
/// `charging` is reported as `false`.
async fn spawn_battery_task(
    battery: &Characteristic,
    battery_tx: UnboundedSender<Battery>,
) -> anyhow::Result<()> {
    // Best-effort initial read so the UI has a value before the first notify.
    if let Ok(bytes) = battery.read().await {
        if let Some(&pct) = bytes.first() {
            let _ = battery_tx.send(Battery { pct, charging: false });
        }
    }
    let mut notify = Box::pin(battery.notify().await?);
    tokio::spawn(async move {
        while let Some(bytes) = notify.next().await {
            match bytes.first() {
                Some(&pct) => {
                    if battery_tx.send(Battery { pct, charging: false }).is_err() {
                        break; // receiver dropped
                    }
                }
                None => eprintln!("ble: empty battery-level notification"),
            }
        }
    });
    Ok(())
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
        // Prefer write-without-response for low-latency LED updates.
        let req = CharacteristicWriteRequest {
            op_type: WriteOp::Command,
            ..Default::default()
        };
        if let Err(e) = self.led.write_ext(&bytes, &req).await {
            eprintln!("ble: LED write failed, marking disconnected: {e}");
            self.connected = false;
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
        // Capped at 30s from here on.
        assert_eq!(backoff_delay(5), Duration::from_secs(30));
        assert_eq!(backoff_delay(10), Duration::from_secs(30));
        // No overflow panic for large attempts.
        assert_eq!(backoff_delay(1000), Duration::from_secs(30));
    }

    #[test]
    fn bt_uuid16_expands_to_base_uuid() {
        // Battery Service 0x180F -> 0000180f-0000-1000-8000-00805f9b34fb.
        assert_eq!(
            bt_uuid16(0x180F),
            Uuid::parse_str("0000180f-0000-1000-8000-00805f9b34fb").unwrap()
        );
        // Battery Level 0x2A19.
        assert_eq!(
            bt_uuid16(0x2A19),
            Uuid::parse_str("00002a19-0000-1000-8000-00805f9b34fb").unwrap()
        );
    }
}

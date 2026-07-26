use std::time::Duration;

use openmicro_proto::ble::{ADV_NAME_PREFIX, OPENMICRO_SERVICE_UUID};

use crate::flash::{self, DeviceState};

pub const BLE_SCAN: Duration = Duration::from_secs(4);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleState {
    OpenMicro,
    StockLike,
    Absent,
    Unavailable,
}

const STOCK_NAME_HINTS: [&str; 3] = ["creator micro", "codex micro", "micro 2"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    Cable,
    Ble,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareKind {
    OpenMicro,
    Stock,
    Bootloader,
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Probe {
    pub usb: DeviceState,
    pub ble: BleState,
}

impl Default for Probe {
    fn default() -> Self {
        Probe { usb: DeviceState::Absent, ble: BleState::Unavailable }
    }
}

impl Probe {
    pub fn connection(self) -> Connection {
        match self.usb {
            DeviceState::Bootloader | DeviceState::NormalDevice => Connection::Cable,
            DeviceState::Absent => match self.ble {
                BleState::OpenMicro | BleState::StockLike => Connection::Ble,
                BleState::Absent | BleState::Unavailable => Connection::None,
            },
        }
    }

    pub fn firmware(self) -> FirmwareKind {
        if self.usb == DeviceState::Bootloader {
            return FirmwareKind::Bootloader;
        }
        if self.ble == BleState::OpenMicro {
            return FirmwareKind::OpenMicro;
        }
        if self.usb == DeviceState::NormalDevice || self.ble == BleState::StockLike {
            return FirmwareKind::Stock;
        }
        FirmwareKind::Unknown
    }

    pub fn any_device(self) -> bool {
        self.connection() != Connection::None
    }
}

pub fn probe() -> Probe {
    let usb = flash::classify_usb(&flash::detect_usb());
    let ble = match usb {
        DeviceState::Bootloader | DeviceState::NormalDevice => BleState::Unavailable,
        DeviceState::Absent => scan_ble(BLE_SCAN),
    };
    Probe { usb, ble }
}

pub fn scan_ble(timeout: Duration) -> BleState {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return BleState::Unavailable;
    };
    rt.block_on(async {
        match tokio::time::timeout(timeout * 2, scan_ble_async(timeout)).await {
            Ok(state) => state,
            Err(_) => BleState::Unavailable,
        }
    })
}

async fn scan_ble_async(timeout: Duration) -> BleState {
    use futures::StreamExt;

    let Ok(session) = bluer::Session::new().await else {
        return BleState::Unavailable;
    };
    let Ok(adapter) = session.default_adapter().await else {
        return BleState::Unavailable;
    };
    if adapter.set_powered(true).await.is_err() {
        return BleState::Unavailable;
    }
    let Ok(mut events) = adapter.discover_devices().await else {
        return BleState::Unavailable;
    };

    let deadline = tokio::time::Instant::now() + timeout;
    let mut best = BleState::Absent;
    loop {
        match tokio::time::timeout_at(deadline, events.next()).await {
            Err(_) | Ok(None) => break,
            Ok(Some(bluer::AdapterEvent::DeviceAdded(addr))) => {
                let Ok(device) = adapter.device(addr) else { continue };
                match classify_device(&device).await {
                    BleState::OpenMicro => return BleState::OpenMicro,
                    BleState::StockLike => best = BleState::StockLike,
                    _ => {}
                }
            }
            Ok(Some(_)) => {}
        }
    }
    best
}

async fn classify_device(device: &bluer::Device) -> BleState {
    if let Ok(Some(uuids)) = device.uuids().await {
        if uuids.contains(&uuid::Uuid::from_u128(OPENMICRO_SERVICE_UUID)) {
            return BleState::OpenMicro;
        }
    }
    let name = device.name().await.ok().flatten().unwrap_or_default();
    BleState::from_name(&name)
}

impl BleState {
    pub fn from_name(name: &str) -> BleState {
        if name.starts_with(ADV_NAME_PREFIX) {
            return BleState::OpenMicro;
        }
        let lower = name.to_ascii_lowercase();
        if STOCK_NAME_HINTS.iter().any(|h| lower.contains(h)) {
            BleState::StockLike
        } else {
            BleState::Absent
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(usb: DeviceState, ble: BleState) -> Probe {
        Probe { usb, ble }
    }

    #[test]
    fn bootloader_is_reported_before_any_firmware_guess() {
        let probe = p(DeviceState::Bootloader, BleState::OpenMicro);
        assert_eq!(probe.firmware(), FirmwareKind::Bootloader);
        assert_eq!(probe.connection(), Connection::Cable);
    }

    #[test]
    fn openmicro_service_over_ble_identifies_our_firmware() {
        let probe = p(DeviceState::Absent, BleState::OpenMicro);
        assert_eq!(probe.firmware(), FirmwareKind::OpenMicro);
        assert_eq!(probe.connection(), Connection::Ble);
    }

    #[test]
    fn vendor_usb_id_means_stock_firmware() {
        let probe = p(DeviceState::NormalDevice, BleState::Unavailable);
        assert_eq!(probe.firmware(), FirmwareKind::Stock);
        assert_eq!(probe.connection(), Connection::Cable);
    }

    #[test]
    fn stock_over_ble_only_is_a_cable_prompt_case() {
        let probe = p(DeviceState::Absent, BleState::StockLike);
        assert_eq!(probe.firmware(), FirmwareKind::Stock);
        assert_eq!(probe.connection(), Connection::Ble, "no cable: flashing impossible");
    }

    #[test]
    fn nothing_anywhere_is_unknown_and_unreachable() {
        let probe = p(DeviceState::Absent, BleState::Absent);
        assert_eq!(probe.firmware(), FirmwareKind::Unknown);
        assert_eq!(probe.connection(), Connection::None);
        assert!(!probe.any_device());
    }

    #[test]
    fn no_bluetooth_stack_is_not_the_same_as_no_device() {
        let probe = p(DeviceState::Absent, BleState::Unavailable);
        assert_eq!(probe.connection(), Connection::None);
        assert_eq!(probe.firmware(), FirmwareKind::Unknown);
    }

    #[test]
    fn cable_wins_over_ble_because_only_it_can_flash() {
        assert_eq!(p(DeviceState::NormalDevice, BleState::OpenMicro).connection(), Connection::Cable);
    }

    #[test]
    fn name_classification() {
        assert_eq!(BleState::from_name("OpenMicro-01"), BleState::OpenMicro);
        assert_eq!(BleState::from_name("Creator Micro 2"), BleState::StockLike);
        assert_eq!(BleState::from_name("CODEX MICRO"), BleState::StockLike);
        assert_eq!(BleState::from_name("Someone's Headphones"), BleState::Absent);
        assert_eq!(BleState::from_name(""), BleState::Absent);
    }

    #[test]
    fn default_probe_knows_nothing() {
        let probe = Probe::default();
        assert!(!probe.any_device());
        assert_eq!(probe.firmware(), FirmwareKind::Unknown);
    }

    #[test]
    fn probe_does_not_panic_without_hardware() {
        let probe = probe();
        assert!(matches!(
            probe.usb,
            DeviceState::Absent | DeviceState::NormalDevice | DeviceState::Bootloader
        ));
    }
}

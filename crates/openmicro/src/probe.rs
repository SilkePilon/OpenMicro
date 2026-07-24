//! What is the macropad doing right now: is it plugged in, is it reachable over
//! Bluetooth, and which firmware is it running?
//!
//! The setup wizard branches entirely on this, so the classification rules are
//! pure functions over a [`Probe`] and the I/O that fills a `Probe` in is kept
//! to one place. The BLE half needs a bounded scan (seconds), so callers run
//! [`probe`] on a worker thread and keep rendering.

use std::time::Duration;

use openmicro_proto::ble::{ADV_NAME_PREFIX, OPENMICRO_SERVICE_UUID};

use crate::flash::{self, DeviceState};

/// How long a single BLE discovery scan runs before giving up.
pub const BLE_SCAN: Duration = Duration::from_secs(4);

/// What a BLE scan found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BleState {
    /// A device advertising the OpenMicro GATT service (or the OpenMicro name
    /// prefix) — this is our firmware, already running.
    OpenMicro,
    /// A device whose advertised name looks like a Creator/Codex Micro but that
    /// does **not** expose the OpenMicro service: the stock firmware.
    /// Heuristic — the stock firmware's advertised name is not documented.
    StockLike,
    /// Scanned successfully, saw no macropad.
    Absent,
    /// No Bluetooth stack available (no adapter, `bluetoothd` not running, or
    /// the scan could not start). Distinct from `Absent`: we learned nothing.
    Unavailable,
}

/// Advertised-name fragments (lowercased) that suggest a stock Micro 2.
const STOCK_NAME_HINTS: [&str; 3] = ["creator micro", "codex micro", "micro 2"];

/// How the host can currently reach the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Connection {
    /// On the USB bus (either running or in bootloader mode).
    Cable,
    /// Only over Bluetooth — flashing is impossible until a cable is attached.
    Ble,
    /// Not reachable at all.
    None,
}

/// Which firmware the device appears to be running.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FirmwareKind {
    /// OpenMicro custom firmware (BLE service present).
    OpenMicro,
    /// The vendor's stock firmware.
    Stock,
    /// In ROM bootloader mode: no firmware is running, and it is ready to flash.
    Bootloader,
    /// Nothing found, or nothing that identifies itself.
    Unknown,
}

/// One point-in-time observation of the device.
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
    /// How we can reach the device. A cable wins over BLE: it is the only
    /// transport that can flash.
    pub fn connection(self) -> Connection {
        match self.usb {
            DeviceState::Bootloader | DeviceState::NormalDevice => Connection::Cable,
            DeviceState::Absent => match self.ble {
                BleState::OpenMicro | BleState::StockLike => Connection::Ble,
                BleState::Absent | BleState::Unavailable => Connection::None,
            },
        }
    }

    /// Best guess at the running firmware.
    ///
    /// Bootloader mode is reported first: no firmware is running at all then,
    /// and it is the state the whole flashing flow is waiting for. Otherwise
    /// the OpenMicro BLE service is the only positive identification we have;
    /// a device enumerating as the vendor's USB product id is stock.
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

    /// True when we can see the device at all.
    pub fn any_device(self) -> bool {
        self.connection() != Connection::None
    }
}

/// Take one observation: enumerate USB, then (only if that found nothing
/// conclusive) scan BLE.
///
/// The BLE scan is skipped whenever USB already answers the question, because
/// it costs seconds and holds the Bluetooth adapter — and a cabled device is
/// exactly the case where the wizard needs to react quickly.
pub fn probe() -> Probe {
    let usb = flash::classify_usb(&flash::detect_usb());
    let ble = match usb {
        // A cabled device tells us everything the wizard branches on.
        DeviceState::Bootloader | DeviceState::NormalDevice => BleState::Unavailable,
        DeviceState::Absent => scan_ble(BLE_SCAN),
    };
    Probe { usb, ble }
}

/// Scan for a BLE macropad, blocking for at most `timeout`.
///
/// Runs its own single-threaded Tokio runtime so the synchronous TUI can call
/// it from a worker thread without an async runtime of its own. Any failure to
/// reach BlueZ is reported as [`BleState::Unavailable`] rather than an error:
/// "we could not look" and "we looked and found nothing" lead to different
/// wizard text, and conflating them would tell the user to plug in a cable for
/// a device that is not even paired.
pub fn scan_ble(timeout: Duration) -> BleState {
    let Ok(rt) = tokio::runtime::Builder::new_current_thread().enable_all().build() else {
        return BleState::Unavailable;
    };
    // Hard outer bound. The discovery loop already stops at `timeout`, but the
    // BlueZ handshake before it (session, adapter, set_powered) is unbounded
    // D-Bus I/O — and a scan that never returns would wedge the wizard's poll
    // loop, which only ever keeps one probe in flight.
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
            // Scan window elapsed, or the event stream ended.
            Err(_) | Ok(None) => break,
            Ok(Some(bluer::AdapterEvent::DeviceAdded(addr))) => {
                let Ok(device) = adapter.device(addr) else { continue };
                match classify_device(&device).await {
                    // A positive OpenMicro identification is final.
                    BleState::OpenMicro => return BleState::OpenMicro,
                    // Keep scanning: a stock-looking device may still turn out
                    // to be sitting next to an OpenMicro one.
                    BleState::StockLike => best = BleState::StockLike,
                    _ => {}
                }
            }
            Ok(Some(_)) => {}
        }
    }
    best
}

/// Classify one discovered BLE device.
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
    /// Classify a device purely from its advertised name.
    ///
    /// Pure, so the (necessarily heuristic) stock-firmware matching is testable
    /// without a Bluetooth adapter.
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
        // Unavailable must not be reported as a reachable BLE connection: the
        // wizard would otherwise tell the user to swap to a cable for a device
        // it never actually saw.
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
        // No Micro 2 attached in CI/dev; probe() must still return a value.
        let probe = probe();
        assert!(matches!(
            probe.usb,
            DeviceState::Absent | DeviceState::NormalDevice | DeviceState::Bootloader
        ));
    }
}

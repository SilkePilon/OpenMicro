//! BLE GATT server: the custom OpenMicro service (LED write / INPUT notify)
//! plus the standard Battery Service, over TrouBLE (`trouble-host`) running
//! on `esp-radio`'s BLE controller.
//!
//! Structural skeleton only (see crate-level docs in `main.rs`) — the exact
//! `trouble-host` 0.6.0 GATT-server builder API (attribute table macros,
//! `GattServer` construction, advertising, and the connection event loop)
//! is written to the shape documented in the TrouBLE examples / esp-radio's
//! "use TrouBLE as the BLE stack" guidance, but is UNVERIFIED — this crate
//! is not compiled here (no Xtensa toolchain, see the firmware README).
//!
//! Version note: `trouble-host` is pinned to 0.6.0, not the crates.io-latest
//! 0.7.0, because 0.7.0 requires `bt-hci ^0.9` while `esp-radio` 0.18.0 (our
//! controller) still pins `bt-hci ^0.8.0` — confirmed incompatible from both
//! crates' published `Cargo.toml`. 0.6.0 requires `bt-hci ^0.8`, matching
//! esp-radio exactly. See the firmware report for the full rationale.

use openmicro_proto::{InputEvent, LedFrame};

/// GATT attribute handles / UUIDs, re-exported from `openmicro-proto` so the
/// host and firmware can never drift.
pub mod uuids {
    pub use openmicro_proto::ble::{
        ADV_NAME_PREFIX, BATTERY_LEVEL_UUID, BATTERY_SERVICE_UUID, INPUT_CHAR_UUID, LED_CHAR_UUID,
        OPENMICRO_SERVICE_UUID,
    };
}

/// Max simultaneous BLE connections. The Micro 2 only needs to serve one
/// host at a time; TrouBLE requires this as a const generic on the stack.
pub const MAX_CONNECTIONS: usize = 1;

/// Max GATT attributes (service + 2 custom chars + battery service/char +
/// the mandatory GAP/GATT service attributes). Sized with headroom.
pub const MAX_ATTRIBUTES: usize = 16;

/// L2CAP MTU. 247 is the common "fits one radio packet after ATT/L2CAP
/// overhead" size and comfortably fits an encoded `LedFrame`
/// (6 slots * (Rgb + Effect-as-u8-ish + brightness) via postcard, well
/// under 100 bytes) or `InputEvent` (a few bytes) in one write/notify.
pub const L2CAP_MTU: usize = 247;

/// Handle to the LED-write characteristic's decoded payload, passed to the
/// LED render task (`leds.rs`) over an embassy channel.
pub struct LedWriteEvent(pub LedFrame);

/// Handle to an outbound INPUT notification, produced by the input task
/// (`input.rs`) and drained by the BLE task to notify subscribers.
pub struct InputNotifyEvent(pub InputEvent);

/// Decode a raw LED-characteristic write payload. Firmware-side mirror of
/// the daemon's encode step — kept here (rather than only relying on
/// `LedFrame::decode` at the call site) so a malformed/short write is a
/// clear, named failure mode in the BLE task's logs.
pub fn decode_led_write(bytes: &[u8]) -> Option<LedFrame> {
    LedFrame::decode(bytes).ok()
}

/// Encode an `InputEvent` for a GATT notification.
///
/// Returns `None` on the (practically unreachable, since `InputEvent` is a
/// small fixed-shape enum) postcard encode failure, so the caller can skip
/// a bad notification instead of panicking the input task.
pub fn encode_input_notify(ev: &InputEvent) -> Option<heapless::Vec<u8, 32>> {
    let bytes = ev.encode().ok()?;
    heapless::Vec::from_slice(&bytes).ok()
}

// ---------------------------------------------------------------------
// Sketch of the real GATT server wiring (NOT compiled — see module docs).
// ---------------------------------------------------------------------
//
// TrouBLE's attribute-table macro roughly looks like (per its documented
// `AttributeServer`/`gatt_server!`-style API as of 0.6.x):
//
// use trouble_host::prelude::*;
//
// #[gatt_server]
// struct Server {
//     openmicro: OpenMicroService,
//     battery: BatteryService,
// }
//
// #[gatt_service(uuid = uuids::OPENMICRO_SERVICE_UUID)]
// struct OpenMicroService {
//     #[characteristic(uuid = uuids::LED_CHAR_UUID, write)]
//     led: [u8; 64],
//     #[characteristic(uuid = uuids::INPUT_CHAR_UUID, notify)]
//     input: [u8; 32],
// }
//
// #[gatt_service(uuid = uuids::BATTERY_SERVICE_UUID)]
// struct BatteryService {
//     #[characteristic(uuid = uuids::BATTERY_LEVEL_UUID, read, notify)]
//     level: u8,
// }
//
// async fn run(controller: impl bt_hci::controller::Controller) -> ! {
//     let resources: HostResources<MAX_CONNECTIONS, MAX_ATTRIBUTES, L2CAP_MTU> =
//         HostResources::new();
//     let stack = trouble_host::new(controller, &mut resources);
//     let server = Server::new_with_config(stack, GapConfig::peripheral(
//         PeripheralConfig { name: uuids::ADV_NAME_PREFIX, appearance: &appearance::GENERIC_KEYBOARD },
//     )).unwrap();
//
//     loop {
//         // advertise, accept a connection, then in the GATT event loop:
//         //   Event::Write { handle, data } if handle == server.openmicro.led.handle => {
//         //       if let Some(frame) = decode_led_write(data) {
//         //           LED_FRAME_CHANNEL.send(LedWriteEvent(frame)).await;
//         //       }
//         //   }
//         // and, driven by INPUT_CHANNEL.receive():
//         //   server.notify(&server.openmicro.input, &conn, &bytes).await.ok();
//     }
// }

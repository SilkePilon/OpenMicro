use openmicro_proto::{InputEvent, LedFrame};

pub mod uuids {
    pub use openmicro_proto::ble::{
        ADV_NAME_PREFIX, BATTERY_LEVEL_UUID, BATTERY_SERVICE_UUID, INPUT_CHAR_UUID, LED_CHAR_UUID,
        OPENMICRO_SERVICE_UUID,
    };
}

pub const MAX_CONNECTIONS: usize = 1;

pub const MAX_ATTRIBUTES: usize = 16;

pub const L2CAP_MTU: usize = 247;

pub struct LedWriteEvent(pub LedFrame);

pub struct InputNotifyEvent(pub InputEvent);

pub fn decode_led_write(bytes: &[u8]) -> Option<LedFrame> {
    LedFrame::decode(bytes).ok()
}

pub fn encode_input_notify(ev: &InputEvent) -> Option<heapless::Vec<u8, 32>> {
    let bytes = ev.encode().ok()?;
    heapless::Vec::from_slice(&bytes).ok()
}

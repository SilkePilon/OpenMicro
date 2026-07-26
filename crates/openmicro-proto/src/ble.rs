pub const OPENMICRO_SERVICE_UUID: u128 = 0x9e7a0001_0000_4000_8000_0026bb765291;
pub const LED_CHAR_UUID: u128 = 0x9e7a0002_0000_4000_8000_0026bb765291;
pub const INPUT_CHAR_UUID: u128 = 0x9e7a0003_0000_4000_8000_0026bb765291;
pub const BATTERY_SERVICE_UUID: u16 = 0x180F;
pub const BATTERY_LEVEL_UUID: u16 = 0x2A19;

pub const ADV_NAME_PREFIX: &str = "OpenMicro";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn service_uuids_are_distinct() {
        let uuids = [OPENMICRO_SERVICE_UUID, LED_CHAR_UUID, INPUT_CHAR_UUID];
        for (i, a) in uuids.iter().enumerate() {
            for b in uuids.iter().skip(i + 1) {
                assert_ne!(a, b, "128-bit BLE UUIDs must be distinct");
            }
        }
        assert_ne!(LED_CHAR_UUID, INPUT_CHAR_UUID);
    }
}

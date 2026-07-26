const RTC_CNTL_OPTION1_REG: *mut u32 = 0x6000_812C as *mut u32;

const FORCE_DOWNLOAD_BOOT: u32 = 1 << 0;

pub fn reboot_to_bootloader() -> ! {
    unsafe {
        core::ptr::write_volatile(RTC_CNTL_OPTION1_REG, FORCE_DOWNLOAD_BOOT);
    }
    esp_hal::system::software_reset()
}

pub fn clear_force_download() {
    unsafe {
        let current = core::ptr::read_volatile(RTC_CNTL_OPTION1_REG);
        core::ptr::write_volatile(RTC_CNTL_OPTION1_REG, current & !FORCE_DOWNLOAD_BOOT);
    }
}

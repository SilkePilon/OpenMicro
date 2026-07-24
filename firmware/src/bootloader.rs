//! Rebooting into the ROM bootloader on command.
//!
//! The stock firmware exposes this as a `sys.bootloader` JSON-RPC over its
//! vendor HID interface, because it enumerates as USB-OTG HID and therefore
//! never puts the USB-Serial-JTAG peripheral on the bus — leaving esptool with
//! nothing to talk to. OpenMicro keeps the capability, by two routes:
//!
//! * **From the host, without any firmware involvement.** This firmware logs
//!   over USB-Serial-JTAG, so that peripheral *is* on the bus, and esptool can
//!   reset the chip into download mode itself (`--before usb-reset`). That path
//!   works even if this firmware has crashed.
//! * **From the device.** [`reboot_to_bootloader`] below, reachable by holding
//!   the power button (see `openmicro_effects::power`) or by a host command
//!   over BLE.
//!
//! Both end in the same place the vendor's does: the force-download-boot bit
//! set, then a reset.

/// `RTC_CNTL_OPTION1_REG`. Setting its low bit tells the ROM to enter download
/// mode on the next boot instead of running the app.
///
/// This register is battery-backed on the Pro model, which is why it is
/// cleared again on the way *out* of download mode (see the host's
/// `wldevice::exit_args`) — left set, the device would re-enter download mode
/// on every boot, including after an unplug.
const RTC_CNTL_OPTION1_REG: *mut u32 = 0x6000_812C as *mut u32;

/// `FORCE_DOWNLOAD_BOOT`.
const FORCE_DOWNLOAD_BOOT: u32 = 1 << 0;

/// Set the force-download bit and reset into the ROM bootloader.
///
/// Never returns. The caller should make sure anything that matters has been
/// persisted first — this is an immediate reset, not a graceful shutdown.
pub fn reboot_to_bootloader() -> ! {
    // SAFETY: a single 32-bit write to a fixed peripheral register that the
    // ROM defines for exactly this purpose. Nothing else in this firmware
    // touches it, and the write is immediately followed by a reset, so there is
    // no ordering hazard with other code.
    unsafe {
        core::ptr::write_volatile(RTC_CNTL_OPTION1_REG, FORCE_DOWNLOAD_BOOT);
    }
    esp_hal::system::software_reset()
}

/// Clear the force-download bit.
///
/// Called early on a normal boot so that a device which entered download mode
/// once does not keep doing so: the bit survives a reset, and on the Pro it
/// survives losing USB power too.
pub fn clear_force_download() {
    // SAFETY: same register, same reasoning as above.
    unsafe {
        let current = core::ptr::read_volatile(RTC_CNTL_OPTION1_REG);
        core::ptr::write_volatile(RTC_CNTL_OPTION1_REG, current & !FORCE_DOWNLOAD_BOOT);
    }
}

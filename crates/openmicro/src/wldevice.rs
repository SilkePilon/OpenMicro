//! Work Louder vendor HID control: the software route into and out of the
//! ESP32-S3 ROM bootloader.
//!
//! The Creator Micro 2 has no usable boot-button workflow. Its firmware
//! enumerates as a composite HID device on its own product id, so the hardware
//! USB-Serial-JTAG peripheral is not on the bus at all and esptool's usual
//! reset dance has nothing to talk to. Work Louder's own tooling instead asks
//! the *firmware* to reboot into download mode, over a vendor HID interface.
//!
//! Wire format, matching `@worklouder/wl-device-kit` and independently
//! confirmed against microbridge's `mb-device` crate, which drives the same
//! hardware:
//!
//! ```text
//! interface: usage page 0xFF00 on VID 0x303A
//! report:    [0]=0x06 report id, [1]=channel, [2]=payload len (0..=61), [3..]=UTF-8
//! payload:   {"method":"sys.bootloader","params":null,"id":<0..999>}
//! ```
//!
//! Coming back out is a different mechanism entirely: once in download mode the
//! device is a plain CDC serial port speaking the esptool protocol, and the
//! only reset that works on USB-Serial-JTAG is the RTC watchdog one.

use std::process::Command;
use std::time::{Duration, Instant};

/// Espressif's USB vendor id, which Work Louder ships under.
pub const WL_VID: u16 = 0x303A;

/// Product id of the Codex Micro — the same hardware under ChatGPT branding.
pub const CODEX_MICRO_PID: u16 = 0x8360;

/// Every product id that means "a supported macropad, running its firmware".
pub const APP_PIDS: [u16; 3] = [0x8297, 0x8298, CODEX_MICRO_PID];

/// Product id of the ESP32-S3 ROM bootloader (USB JTAG/serial debug unit).
pub const BOOTLOADER_PID: u16 = 0x1001;

/// Vendor-specific HID usage page carrying the JSON-RPC channel.
pub const WL_USAGE_PAGE: u16 = 0xFF00;

/// HID report id used for all Work Louder vendor traffic.
pub const REPORT_ID: u8 = 0x06;

/// Channel byte for the JSON-RPC stream (channel 1 is device debug logging).
pub const CHANNEL_RPC: u8 = 2;

/// Bytes of payload that fit in one report after the 3-byte header.
pub const MAX_CHUNK: usize = 61;

/// Size of a full outgoing report, report id included.
pub const REPORT_SIZE: usize = 64;

/// The RPC method that reboots the device into ROM download mode.
pub const BOOTLOADER_METHOD: &str = "sys.bootloader";

/// `RTC_CNTL_OPTION1_REG`. Its force-download-boot bit is what `sys.bootloader`
/// sets, and on the battery-backed Pro model it survives an unplug — so it must
/// be cleared explicitly, or the device re-enters download mode on every boot.
pub const RTC_CNTL_OPTION1_REG: u32 = 0x6000_812C;

/// How long to wait for the device to drop off USB and come back as the ROM
/// bootloader after being asked to reboot.
pub const BOOTLOADER_APPEAR_TIMEOUT: Duration = Duration::from_secs(10);

/// True for a product id that means the macropad is running its firmware.
pub fn is_app_pid(pid: u16) -> bool {
    APP_PIDS.contains(&pid)
}

/// Human label for a product id, for log lines.
pub fn product_name(pid: u16) -> &'static str {
    match pid {
        CODEX_MICRO_PID => "Codex Micro",
        0x8297 | 0x8298 => "Creator Micro 2",
        BOOTLOADER_PID => "ESP32-S3 ROM bootloader",
        _ => "unknown Work Louder device",
    }
}

/// Build a JSON-RPC request in the exact shape the firmware expects.
///
/// Hand-built rather than serialized from a struct on purpose: the firmware
/// wants the compact form with **no** `"jsonrpc":"2.0"` member, these keys in
/// this order, and it rejects ids of 1000 or more.
pub fn rpc_request(method: &str, id: u16) -> String {
    format!(r#"{{"method":"{method}","params":null,"id":{}}}"#, id % 1000)
}

/// Frame a UTF-8 message into 64-byte HID reports on the RPC channel.
///
/// Messages longer than [`MAX_CHUNK`] are split across consecutive reports; an
/// empty message still produces one zero-length report, matching the vendor
/// implementation.
pub fn frame_rpc(message: &str) -> Vec<[u8; REPORT_SIZE]> {
    let bytes = message.as_bytes();
    if bytes.is_empty() {
        let mut report = [0u8; REPORT_SIZE];
        report[0] = REPORT_ID;
        report[1] = CHANNEL_RPC;
        return vec![report];
    }
    bytes
        .chunks(MAX_CHUNK)
        .map(|chunk| {
            let mut report = [0u8; REPORT_SIZE];
            report[0] = REPORT_ID;
            report[1] = CHANNEL_RPC;
            report[2] = chunk.len() as u8;
            report[3..3 + chunk.len()].copy_from_slice(chunk);
            report
        })
        .collect()
}

/// An id for one request. Not random — just varied enough that a reply can be
/// matched to its request within a session, and always under the firmware's
/// limit of 1000.
fn next_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static COUNTER: AtomicU16 = AtomicU16::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    (u32::from(std::process::id() as u16).wrapping_add(u32::from(n)) % 1000) as u16
}

/// What the USB bus says about the macropad right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbMode {
    /// Running its firmware, so RPC-controllable.
    App(u16),
    /// In ROM download mode, so flashable.
    Bootloader,
    /// Not on the bus.
    Absent,
}

/// Classify the macropad's USB presence from the `(vid, pid)` pairs on the bus.
///
/// Bootloader wins if both are somehow seen, since that is the state every
/// flashing operation cares about.
///
/// **`BOOTLOADER_PID` is ambiguous** and this function cannot resolve it: id
/// `303a:1001` is both the ESP32-S3 ROM bootloader *and* any firmware exposing a
/// USB-Serial-JTAG console, which OpenMicro's does for its logs. Deciding
/// between them needs I/O, so it happens in [`usb_mode`]; this stays a pure
/// function over ids and reports the conservative answer.
pub fn classify(ids: &[(u16, u16)]) -> UsbMode {
    if ids.contains(&(WL_VID, BOOTLOADER_PID)) {
        return UsbMode::Bootloader;
    }
    for (vid, pid) in ids {
        if *vid == WL_VID && is_app_pid(*pid) {
            return UsbMode::App(*pid);
        }
    }
    UsbMode::Absent
}

/// Resolve the `303a:1001` ambiguity by asking the device.
///
/// Given [`classify`]'s answer and whether OpenMicro firmware replied on the
/// serial port, produce the real mode. Split out from the I/O so the rule itself
/// is testable: a running device must not be reported as being in bootloader
/// mode, which is the bug this fixes — the TUI used to say "bootloader mode"
/// forever, including immediately after being told to go back to normal.
pub fn resolve_ambiguous(mode: UsbMode, firmware_answered: bool) -> UsbMode {
    match mode {
        // The ROM bootloader never answers, so a reply means firmware is running.
        UsbMode::Bootloader if firmware_answered => UsbMode::App(BOOTLOADER_PID),
        other => other,
    }
}

/// Current USB mode straight from sysfs, with the `303a:1001` ambiguity left
/// unresolved.
///
/// Cheap and side-effect free. Use this anywhere the answer only has to
/// distinguish "something is on the bus" from "nothing is", and in any loop:
/// [`usb_mode`] writes to the device and can wait up to
/// [`crate::display::IDENTIFY_TIMEOUT`], which is not something to do every
/// 250 ms.
pub fn usb_mode_raw() -> UsbMode {
    classify(&crate::flash::detect_usb())
}

/// Current USB mode, read from sysfs and — when the id is ambiguous — confirmed
/// by asking the device.
///
/// This one talks to the device, so it belongs in the paths that report state to
/// the user, not in polling loops or in the middle of a flash sequence.
pub fn usb_mode() -> UsbMode {
    let seen = usb_mode_raw();
    // Only pay for the probe when the id cannot settle it; the check costs up to
    // ~600 ms of waiting for a bootloader that will never reply.
    if seen == UsbMode::Bootloader {
        return resolve_ambiguous(seen, crate::display::firmware_answers());
    }
    seen
}

/// esptool argv that just talks to whatever is on the port, without resetting
/// it. Succeeds only if a ROM bootloader is actually listening.
pub fn sync_args(port: Option<&str>) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), crate::flash::CHIP.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("--before".to_string());
    args.push("no-reset".to_string());
    args.push("--after".to_string());
    args.push("no-reset".to_string());
    args.push("chip-id".to_string());
    args
}

/// esptool argv that resets the chip into download mode over USB-Serial-JTAG.
///
/// This is the route that works on *OpenMicro* firmware, which — unlike the
/// vendor's — exposes the USB-Serial-JTAG peripheral, so esptool can drive the
/// reset itself and needs no cooperation from the running firmware.
pub fn usb_reset_args(port: Option<&str>) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), crate::flash::CHIP.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("--before".to_string());
    args.push("usb-reset".to_string());
    args.push("--after".to_string());
    args.push("no-reset".to_string());
    args.push("chip-id".to_string());
    args
}

/// Whether a ROM bootloader is really listening.
///
/// `303a:1001` is ambiguous: it is what the ROM download mode enumerates as,
/// **and** what any firmware exposing the USB-Serial-JTAG console looks like —
/// including OpenMicro's own. Presence on the bus is therefore not evidence of
/// download mode; only a successful esptool sync is.
pub fn download_mode_responds(port: Option<&str>) -> bool {
    let Some(esptool) = crate::flash::esptool_path() else {
        return false;
    };
    Command::new(&esptool)
        .args(sync_args(port))
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Ask the running firmware to reboot into ROM download mode.
///
/// Returns progress lines for the UI. Succeeds when the bootloader actually
/// shows up on USB — *not* when the write succeeds — because the device
/// frequently drops off the bus before it can answer, so a write error is not
/// evidence of failure. This mirrors what Work Louder's own tool does.
pub fn enter_bootloader() -> Result<Vec<String>, String> {
    let mut log = Vec::new();

    // Raw, not the probing form: the `download_mode_responds` check just below
    // already resolves the ambiguity, and better — it asks the bootloader
    // directly instead of inferring from our firmware's silence.
    match usb_mode_raw() {
        UsbMode::Bootloader => {
            // Could be the ROM bootloader, or could be OpenMicro firmware
            // exposing the same USB-Serial-JTAG identity. Ask before assuming.
            if download_mode_responds(None) {
                log.push("already in bootloader mode.".to_string());
                return Ok(log);
            }
            log.push(
                "a device is present over USB-Serial-JTAG but not in download mode \
                 (OpenMicro firmware looks like this); resetting it."
                    .to_string(),
            );
            return usb_reset_into_bootloader(log);
        }
        UsbMode::Absent => {
            return Err("no macropad found on USB. Connect it with a data-capable cable \
                        (a charge-only cable enumerates nothing) and try again."
                .to_string());
        }
        UsbMode::App(pid) => log.push(format!("found {} on USB.", product_name(pid))),
    }

    let request = rpc_request(BOOTLOADER_METHOD, next_id());
    log.push(format!("sending {BOOTLOADER_METHOD}"));
    let write_result = send_rpc(&request);

    log.push("waiting for the device to re-enumerate".to_string());
    let deadline = Instant::now() + BOOTLOADER_APPEAR_TIMEOUT;
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(250));
        if usb_mode_raw() == UsbMode::Bootloader {
            log.push("device is in bootloader mode.".to_string());
            return Ok(log);
        }
    }

    // It never came back, so now the write error (if there was one) is the
    // useful part of the story.
    match write_result {
        Err(e) => Err(format!(
            "could not put the device into bootloader mode: {e}\n\
             It stayed on USB in its normal mode."
        )),
        Ok(()) => Err(format!(
            "the device accepted {BOOTLOADER_METHOD} but did not come back as a bootloader \
             within {}s. Unplug and replug it, then try again.",
            BOOTLOADER_APPEAR_TIMEOUT.as_secs()
        )),
    }
}

/// Reset a USB-Serial-JTAG device into download mode with esptool.
///
/// Used for OpenMicro's own firmware, which needs no vendor RPC because the
/// peripheral esptool drives is on the bus. Works even when the firmware has
/// crashed, which is exactly when it is most needed.
fn usb_reset_into_bootloader(mut log: Vec<String>) -> Result<Vec<String>, String> {
    let esptool = crate::flash::esptool_path().ok_or_else(|| {
        "resetting into bootloader mode needs esptool. Install it: pip install esptool."
            .to_string()
    })?;
    let output = Command::new(&esptool)
        .args(usb_reset_args(None))
        .output()
        .map_err(|e| format!("failed to launch esptool: {e}"))?;

    if download_mode_responds(None) {
        log.push("device is in bootloader mode.".to_string());
        return Ok(log);
    }
    let mut lines = crate::flash::combine_output(&output.stdout, &output.stderr);
    lines.push(
        "the device did not enter download mode. If it is running OpenMicro, hold the \
         power button for about 8 seconds to force it."
            .to_string(),
    );
    Err(lines.join("\n"))
}

/// Write one framed RPC request to the vendor HID interface.
///
/// Tries the interface whose usage page is the vendor one first; if the HID
/// backend does not report usage pages (which happens on some Linux setups),
/// falls back to every other interface of a matching device, since writing to
/// the wrong one merely errors.
fn send_rpc(request: &str) -> Result<(), String> {
    let api = hidapi::HidApi::new().map_err(|e| format!("cannot open HID: {e}"))?;

    let mut candidates: Vec<_> = api
        .device_list()
        .filter(|d| d.vendor_id() == WL_VID && is_app_pid(d.product_id()))
        .collect();
    if candidates.is_empty() {
        return Err("no Work Louder HID interface found".to_string());
    }
    candidates.sort_by_key(|d| u8::from(d.usage_page() != WL_USAGE_PAGE));

    let reports = frame_rpc(request);
    let mut last_error = "no interface accepted the write".to_string();
    for info in candidates {
        let device = match info.open_device(&api) {
            Ok(d) => d,
            Err(e) => {
                last_error =
                    format!("{e} — check you have permission to open hidraw devices");
                continue;
            }
        };
        match reports.iter().try_for_each(|r| device.write(r).map(|_| ())) {
            Ok(()) => return Ok(()),
            Err(e) => last_error = e.to_string(),
        }
    }
    Err(last_error)
}

/// Bring the device back out of download mode, without unplugging it.
///
/// Two steps, both required, in one esptool invocation: clear the
/// battery-backed force-download-boot bit so it does not simply re-enter, then
/// perform an RTC **watchdog** reset. A plain reset does not re-sample the boot
/// straps on USB-Serial-JTAG, and this board exposes no DTR/RTS lines for the
/// classic auto-reset, so `--after watchdog-reset` is the only thing that works.
pub fn exit_bootloader(port: Option<&str>) -> Result<Vec<String>, String> {
    // Raw, not the probing form. This runs immediately after flashing, when the
    // device is sitting in the ROM bootloader and therefore silent — and the
    // question here is only "is anything on the bus", which the id answers on
    // its own. Probing here is what hung the wizard at "Starting the new
    // firmware": the step that exists to *get out* of the bootloader was waiting
    // on a reply that a bootloader never sends.
    if usb_mode_raw() == UsbMode::Absent {
        return Ok(vec!["no device on USB; nothing to do.".to_string()]);
    }
    let esptool = crate::flash::esptool_path().ok_or_else(|| {
        "leaving bootloader mode needs esptool. Install it: pip install esptool.".to_string()
    })?;

    let output = Command::new(&esptool)
        .args(exit_args(port))
        .output()
        .map_err(|e| format!("failed to launch esptool: {e}"))?;
    let mut lines = crate::flash::combine_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        lines.push(format!(
            "esptool exited with {}.",
            crate::flash::exit_desc(output.status.code())
        ));
        return Err(lines.join("\n"));
    }
    lines.push("device reset out of bootloader mode.".to_string());
    Ok(lines)
}

/// esptool argv that clears the force-download bit and watchdog-resets.
///
/// Pure, so the exact invocation is pinned by a test: getting `--after` wrong
/// leaves the device stuck re-entering download mode on every boot.
pub fn exit_args(port: Option<&str>) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), crate::flash::CHIP.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("--before".to_string());
    args.push("usb-reset".to_string());
    args.push("--after".to_string());
    args.push("watchdog-reset".to_string());
    args.push("write-mem".to_string());
    args.push(format!("0x{RTC_CNTL_OPTION1_REG:X}"));
    args.push("0".to_string());
    args
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rpc_request_is_the_compact_form_the_firmware_wants() {
        // No "jsonrpc" member, this key order, id under 1000.
        assert_eq!(
            rpc_request("sys.bootloader", 7),
            r#"{"method":"sys.bootloader","params":null,"id":7}"#
        );
    }

    #[test]
    fn rpc_ids_are_kept_below_the_firmware_limit() {
        assert!(rpc_request("sys.version", 1000).ends_with(r#""id":0}"#));
        assert!(rpc_request("sys.version", 1234).ends_with(r#""id":234}"#));
        for _ in 0..2000 {
            assert!(next_id() < 1000);
        }
    }

    #[test]
    fn framing_matches_the_vendor_layout() {
        let msg = rpc_request(BOOTLOADER_METHOD, 1);
        let reports = frame_rpc(&msg);
        assert_eq!(reports.len(), 1, "the bootloader request fits in one report");
        assert_eq!(reports[0][0], REPORT_ID);
        assert_eq!(reports[0][1], CHANNEL_RPC);
        assert_eq!(reports[0][2] as usize, msg.len());
        assert_eq!(&reports[0][3..3 + msg.len()], msg.as_bytes());
        assert_eq!(reports[0].len(), REPORT_SIZE);
        // Trailing bytes are zero-padded, not left as stack garbage.
        assert!(reports[0][3 + msg.len()..].iter().all(|b| *b == 0));
    }

    #[test]
    fn framing_splits_long_messages_at_61_bytes() {
        let reports = frame_rpc(&"x".repeat(130));
        assert_eq!(reports.len(), 3);
        assert_eq!(reports[0][2], 61);
        assert_eq!(reports[1][2], 61);
        assert_eq!(reports[2][2], 8);
    }

    #[test]
    fn framing_an_empty_message_still_sends_one_report() {
        let reports = frame_rpc("");
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0][2], 0);
    }

    #[test]
    fn classify_prefers_bootloader_over_the_app() {
        assert_eq!(classify(&[(WL_VID, BOOTLOADER_PID)]), UsbMode::Bootloader);
        assert_eq!(
            classify(&[(WL_VID, 0x8298), (WL_VID, BOOTLOADER_PID)]),
            UsbMode::Bootloader
        );
    }

    #[test]
    fn classify_recognises_every_supported_product() {
        for pid in APP_PIDS {
            assert_eq!(classify(&[(WL_VID, pid)]), UsbMode::App(pid), "pid {pid:#x}");
        }
    }

    #[test]
    fn classify_ignores_unrelated_devices() {
        assert_eq!(classify(&[(0x1234, 0x5678)]), UsbMode::Absent);
        assert_eq!(classify(&[]), UsbMode::Absent);
        // Same vendor id, but not one of ours — Espressif ships a lot of things.
        assert_eq!(classify(&[(WL_VID, 0x4001)]), UsbMode::Absent);
    }

    #[test]
    fn exit_args_clear_the_force_download_bit_and_watchdog_reset() {
        let args = exit_args(None);
        // `--before usb-reset` rather than `no-reset`: by the time this runs
        // the device may already have rebooted (a flash resets on its way
        // out), and `no-reset` against a device that is not sitting in
        // download mode just times out.
        assert!(args.windows(2).any(|w| w == ["--before", "usb-reset"]), "{args:?}");
        // A plain hard reset does NOT work on USB-Serial-JTAG.
        assert!(args.windows(2).any(|w| w == ["--after", "watchdog-reset"]), "{args:?}");
        assert!(args.contains(&"write-mem".to_string()));
        assert!(args.contains(&"0x6000812C".to_string()), "{args:?}");
        assert_eq!(args.last().unwrap(), "0", "the register is cleared, not set");
    }

    #[test]
    fn sync_args_never_reset_the_chip() {
        // This runs against a device that may be mid-boot; resetting it would
        // change the very state we are trying to observe.
        let args = sync_args(None);
        assert!(args.windows(2).any(|w| w == ["--before", "no-reset"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--after", "no-reset"]), "{args:?}");
    }

    #[test]
    fn usb_reset_args_use_the_serial_jtag_reset() {
        // The route that works on OpenMicro firmware, which exposes
        // USB-Serial-JTAG; the vendor's firmware does not, hence its HID RPC.
        let args = usb_reset_args(Some("/dev/ttyACM0"));
        assert!(args.windows(2).any(|w| w == ["--before", "usb-reset"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--port", "/dev/ttyACM0"]), "{args:?}");
    }

    #[test]
    fn download_mode_is_not_inferred_from_the_usb_id_alone() {
        // 303a:1001 is what the ROM bootloader AND any firmware exposing the
        // USB-Serial-JTAG console look like — including our own. Classification
        // still reports Bootloader for the id, but the *decision* to flash must
        // go through an esptool sync, which is what `download_mode_responds`
        // is for.
        assert_eq!(classify(&[(WL_VID, BOOTLOADER_PID)]), UsbMode::Bootloader);
        // No device attached here, so nothing can answer a sync.
        assert!(!download_mode_responds(Some("/dev/definitely-not-a-port")));
    }

    #[test]
    fn exit_args_pass_an_explicit_port_through() {
        let args = exit_args(Some("/dev/ttyACM0"));
        assert!(args.windows(2).any(|w| w == ["--port", "/dev/ttyACM0"]));
    }

    #[test]
    fn a_device_that_answers_is_running_not_in_the_bootloader() {
        // The reported bug: id 303a:1001 is shared by the ROM bootloader and our
        // own firmware's serial console, so the TUI insisted the device was in
        // bootloader mode even right after being switched back to normal.
        assert_eq!(
            resolve_ambiguous(UsbMode::Bootloader, true),
            UsbMode::App(BOOTLOADER_PID),
            "firmware answered, so it is running"
        );
    }

    #[test]
    fn a_silent_device_on_the_ambiguous_id_is_still_the_bootloader() {
        assert_eq!(resolve_ambiguous(UsbMode::Bootloader, false), UsbMode::Bootloader);
    }

    #[test]
    fn resolving_never_changes_an_unambiguous_answer() {
        // An app PID or an absent device needs no probe, and a stray reply must
        // not be able to rewrite either.
        for mode in [UsbMode::App(APP_PIDS[0]), UsbMode::Absent] {
            assert_eq!(resolve_ambiguous(mode, true), mode);
            assert_eq!(resolve_ambiguous(mode, false), mode);
        }
    }

    #[test]
    fn product_names_cover_every_id_we_advertise() {
        for pid in APP_PIDS {
            assert!(!product_name(pid).contains("unknown"), "pid {pid:#x}");
        }
        assert!(product_name(BOOTLOADER_PID).contains("bootloader"));
        assert!(product_name(0x0000).contains("unknown"));
    }

    #[test]
    fn usb_mode_does_not_panic_without_hardware() {
        // No macropad attached in CI; this must still return a value.
        let _ = usb_mode();
    }

    #[test]
    fn the_raw_mode_check_does_no_device_io() {
        // `usb_mode_raw` exists to be safe inside loops and inside the flash
        // sequence, which means it must not talk to the device. Reading sysfs
        // takes microseconds; an identify probe takes up to IDENTIFY_TIMEOUT,
        // so the gap between the two is enormous and easy to assert on.
        let started = std::time::Instant::now();
        let _ = usb_mode_raw();
        let took = started.elapsed();
        assert!(
            took < crate::display::IDENTIFY_TIMEOUT / 2,
            "usb_mode_raw took {took:?}; it is probing the device, which callers rely on it not doing"
        );
    }

    /// Hardware round trip: into the bootloader and back out.
    ///
    /// Ignored by default — needs a real device, and resets it:
    ///
    /// ```text
    /// cargo test -p openmicro -- --ignored --nocapture bootloader_and_back
    /// ```
    ///
    /// This is here because the wizard hung at "Starting the new firmware", the
    /// one step whose job is to get the device *out* of the bootloader, and no
    /// unit test could have caught it: the cause was a blocking read on a
    /// character device, and the tests stood in with ordinary files, which
    /// report end-of-file at once. Only real hardware is silent the way a ROM
    /// bootloader is silent. So this asserts the thing that cannot be faked —
    /// that the sequence finishes at all.
    #[test]
    #[ignore = "needs a device on USB, and resets it"]
    fn bootloader_and_back() {
        use std::time::{Duration, Instant};
        /// Generous, but far below "forever". The bug was unbounded, so any
        /// finite bound catches it.
        const BUDGET: Duration = Duration::from_secs(90);

        assert_ne!(usb_mode_raw(), UsbMode::Absent, "no device on USB");

        let log = enter_bootloader().expect("could not enter the bootloader");
        println!("enter_bootloader:\n  {}", log.join("\n  "));

        let started = Instant::now();
        let log = exit_bootloader(None).expect("could not leave the bootloader");
        let took = started.elapsed();
        println!("exit_bootloader ({took:?}):\n  {}", log.join("\n  "));
        assert!(took < BUDGET, "leaving the bootloader took {took:?}; it is blocking, not working");

        // And firmware is genuinely running again, not merely reset — the USB id
        // cannot tell us that, since the ROM bootloader shares it.
        std::thread::sleep(Duration::from_secs(3));
        assert!(
            crate::display::firmware_answers(),
            "came out of the bootloader but no firmware answered"
        );
    }
}

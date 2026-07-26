use std::process::Command;
use std::time::{Duration, Instant};

pub const WL_VID: u16 = 0x303A;

pub const CODEX_MICRO_PID: u16 = 0x8360;

pub const APP_PIDS: [u16; 3] = [0x8297, 0x8298, CODEX_MICRO_PID];

pub const BOOTLOADER_PID: u16 = 0x1001;

pub const WL_USAGE_PAGE: u16 = 0xFF00;

pub const REPORT_ID: u8 = 0x06;

pub const CHANNEL_RPC: u8 = 2;

pub const MAX_CHUNK: usize = 61;

pub const REPORT_SIZE: usize = 64;

pub const BOOTLOADER_METHOD: &str = "sys.bootloader";

pub const RTC_CNTL_OPTION1_REG: u32 = 0x6000_812C;

pub const BOOTLOADER_APPEAR_TIMEOUT: Duration = Duration::from_secs(10);

pub fn is_app_pid(pid: u16) -> bool {
    APP_PIDS.contains(&pid)
}

pub fn product_name(pid: u16) -> &'static str {
    match pid {
        CODEX_MICRO_PID => "Codex Micro",
        0x8297 | 0x8298 => "Creator Micro 2",
        BOOTLOADER_PID => "ESP32-S3 ROM bootloader",
        _ => "unknown Work Louder device",
    }
}

pub fn rpc_request(method: &str, id: u16) -> String {
    format!(r#"{{"method":"{method}","params":null,"id":{}}}"#, id % 1000)
}

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

fn next_id() -> u16 {
    use std::sync::atomic::{AtomicU16, Ordering};
    static COUNTER: AtomicU16 = AtomicU16::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    (u32::from(std::process::id() as u16).wrapping_add(u32::from(n)) % 1000) as u16
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsbMode {
    App(u16),
    Bootloader,
    Absent,
}

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

pub fn resolve_ambiguous(mode: UsbMode, firmware_answered: bool) -> UsbMode {
    match mode {
        UsbMode::Bootloader if firmware_answered => UsbMode::App(BOOTLOADER_PID),
        other => other,
    }
}

pub fn usb_mode_raw() -> UsbMode {
    classify(&crate::flash::detect_usb())
}

pub fn usb_mode() -> UsbMode {
    let seen = usb_mode_raw();
    if seen != UsbMode::Bootloader {
        return seen;
    }
    if crate::daemon::is_running() {
        return resolve_ambiguous(seen, true);
    }
    resolve_ambiguous(seen, crate::display::firmware_answers())
}

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

pub fn enter_bootloader() -> Result<Vec<String>, String> {
    let mut log = Vec::new();

    match usb_mode_raw() {
        UsbMode::Bootloader => {
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

pub fn exit_bootloader(port: Option<&str>) -> Result<Vec<String>, String> {
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
        assert_eq!(classify(&[(WL_VID, 0x4001)]), UsbMode::Absent);
    }

    #[test]
    fn exit_args_clear_the_force_download_bit_and_watchdog_reset() {
        let args = exit_args(None);
        assert!(args.windows(2).any(|w| w == ["--before", "usb-reset"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--after", "watchdog-reset"]), "{args:?}");
        assert!(args.contains(&"write-mem".to_string()));
        assert!(args.contains(&"0x6000812C".to_string()), "{args:?}");
        assert_eq!(args.last().unwrap(), "0", "the register is cleared, not set");
    }

    #[test]
    fn sync_args_never_reset_the_chip() {
        let args = sync_args(None);
        assert!(args.windows(2).any(|w| w == ["--before", "no-reset"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--after", "no-reset"]), "{args:?}");
    }

    #[test]
    fn usb_reset_args_use_the_serial_jtag_reset() {
        let args = usb_reset_args(Some("/dev/ttyACM0"));
        assert!(args.windows(2).any(|w| w == ["--before", "usb-reset"]), "{args:?}");
        assert!(args.windows(2).any(|w| w == ["--port", "/dev/ttyACM0"]), "{args:?}");
    }

    #[test]
    fn download_mode_is_not_inferred_from_the_usb_id_alone() {
        assert_eq!(classify(&[(WL_VID, BOOTLOADER_PID)]), UsbMode::Bootloader);
        assert!(!download_mode_responds(Some("/dev/definitely-not-a-port")));
    }

    #[test]
    fn exit_args_pass_an_explicit_port_through() {
        let args = exit_args(Some("/dev/ttyACM0"));
        assert!(args.windows(2).any(|w| w == ["--port", "/dev/ttyACM0"]));
    }

    #[test]
    fn a_device_that_answers_is_running_not_in_the_bootloader() {
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
        let _ = usb_mode();
    }

    #[test]
    fn the_raw_mode_check_does_no_device_io() {
        let started = std::time::Instant::now();
        let _ = usb_mode_raw();
        let took = started.elapsed();
        assert!(
            took < crate::display::IDENTIFY_TIMEOUT / 2,
            "usb_mode_raw took {took:?}; it is probing the device, which callers rely on it not doing"
        );
    }

    #[test]
    #[ignore = "needs a device on USB, and resets it"]
    fn bootloader_and_back() {
        use std::time::{Duration, Instant};
        const BUDGET: Duration = Duration::from_secs(90);

        assert_ne!(usb_mode_raw(), UsbMode::Absent, "no device on USB");

        let log = enter_bootloader().expect("could not enter the bootloader");
        println!("enter_bootloader:\n  {}", log.join("\n  "));

        let started = Instant::now();
        let log = exit_bootloader(None).expect("could not leave the bootloader");
        let took = started.elapsed();
        println!("exit_bootloader ({took:?}):\n  {}", log.join("\n  "));
        assert!(took < BUDGET, "leaving the bootloader took {took:?}; it is blocking, not working");

        std::thread::sleep(Duration::from_secs(3));
        assert!(
            crate::display::firmware_answers(),
            "came out of the bootloader but no firmware answered"
        );
    }
}

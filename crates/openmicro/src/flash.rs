//! Firmware flashing engine for the Creator Micro 2 (ESP32-S3).
//!
//! Split into pure, unit-tested helpers (image resolution, USB device
//! classification, esptool argv construction, esptool discovery) and a single
//! side-effecting [`flash`] driver that stitches them together and streams a
//! real `esptool` subprocess to the terminal.
//!
//! Honesty contract: on this machine the firmware image is not built (no
//! Xtensa toolchain) and the device cannot be put in bootloader mode
//! unattended, so [`flash`] is *designed* to stop with a clear, actionable
//! error at the first missing prerequisite. It NEVER reports a successful
//! flash it did not perform — the real write is the user's on-hardware step.
//
// NOTE: this `#![allow(dead_code)]` covers the interval between landing the
// engine (this commit) and wiring its callers in the `openmicro flash` CLI and
// the TUI installer screen. It is removed once both call sites exist.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// USB VID:PID the Micro 2 enumerates as in ROM bootloader / download mode
/// (Espressif USB JTAG/serial debug unit).
pub const BOOTLOADER_VID_PID: (u16, u16) = (0x303a, 0x1001);

/// USB VID:PID the Micro 2 enumerates as running the stock/OpenMicro app.
pub const NORMAL_VID_PID: (u16, u16) = (0x303a, 0x8298);

/// Default flashable-image path produced by `cargo build --release` in
/// `firmware/`, relative to the repo root (or the current directory).
pub const DEFAULT_IMAGE_REL: &str =
    "firmware/target/xtensa-esp32s3-none-elf/release/openmicro-fw";

/// esptool chip argument for this board.
pub const CHIP: &str = "esp32s3";

/// What (if anything) OpenMicro sees on the USB bus for the Micro 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    /// In ROM bootloader / download mode — ready to be flashed.
    Bootloader,
    /// Enumerated as the normal application device (not flashable as-is).
    NormalDevice,
    /// No Micro 2 seen on the bus at all.
    Absent,
}

/// Resolve the firmware image to flash.
///
/// `explicit` (from `--image`) wins if given, but must exist. Otherwise the
/// default build output ([`DEFAULT_IMAGE_REL`]) is looked up relative to the
/// repo root (walking up for a `firmware/` dir) and to the current directory.
/// If nothing is found, returns an `Err` pointing at the build instructions.
pub fn resolve_image(explicit: Option<&Path>) -> Result<PathBuf, String> {
    if let Some(p) = explicit {
        if p.exists() {
            return Ok(p.to_path_buf());
        }
        return Err(format!(
            "firmware image not found: {} (check the --image path)",
            p.display()
        ));
    }

    for base in candidate_roots() {
        let candidate = base.join(DEFAULT_IMAGE_REL);
        if candidate.exists() {
            return Ok(candidate);
        }
    }

    Err(format!(
        "no firmware image found (looked for {DEFAULT_IMAGE_REL}). \
         Build it first: cd firmware && cargo build --release — see firmware/README.md \
         for the Xtensa toolchain setup."
    ))
}

/// Candidate base directories to resolve [`DEFAULT_IMAGE_REL`] against: the
/// current directory plus each ancestor that contains a `firmware/` dir.
fn candidate_roots() -> Vec<PathBuf> {
    let mut roots = vec![PathBuf::from(".")];
    if let Ok(cwd) = std::env::current_dir() {
        let mut dir: Option<&Path> = Some(cwd.as_path());
        while let Some(d) = dir {
            if d.join("firmware").is_dir() {
                roots.push(d.to_path_buf());
            }
            dir = d.parent();
        }
    }
    roots
}

/// Classify the Micro 2's USB presence from a list of `(vid, pid)` pairs.
/// Bootloader takes priority over the normal device if both are somehow seen.
pub fn classify_usb(ids: &[(u16, u16)]) -> DeviceState {
    if ids.contains(&BOOTLOADER_VID_PID) {
        DeviceState::Bootloader
    } else if ids.contains(&NORMAL_VID_PID) {
        DeviceState::NormalDevice
    } else {
        DeviceState::Absent
    }
}

/// Enumerate `(vid, pid)` pairs from `/sys/bus/usb/devices/*/{idVendor,idProduct}`.
///
/// Pure sysfs reads, no external command. Unreadable/malformed entries are
/// skipped. Returns an empty vec on any platform without that sysfs tree.
pub fn detect_usb() -> Vec<(u16, u16)> {
    let mut out = Vec::new();
    let entries = match std::fs::read_dir("/sys/bus/usb/devices") {
        Ok(e) => e,
        Err(_) => return out,
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let vid = read_hex_u16(&path.join("idVendor"));
        let pid = read_hex_u16(&path.join("idProduct"));
        if let (Some(v), Some(p)) = (vid, pid) {
            out.push((v, p));
        }
    }
    out
}

/// Read a sysfs 4-hex-digit id file (e.g. `303a`) into a `u16`.
fn read_hex_u16(path: &Path) -> Option<u16> {
    let s = std::fs::read_to_string(path).ok()?;
    u16::from_str_radix(s.trim(), 16).ok()
}

/// Build the `esptool` argument vector to flash `image`.
///
/// Layout: a single merged image written at offset `0x0` (bootloader `0x0`,
/// partition table `0x8000`, app `0x10000` are all inside it) per the flash
/// layout in `docs/hardware/creator-micro-2-pinout-research.md`.
pub fn esptool_args(chip: &str, port: Option<&str>, image: &Path) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), chip.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("write_flash".to_string());
    args.push("0x0".to_string());
    args.push(image.display().to_string());
    args
}

/// Locate an `esptool` / `esptool.py` executable: honor `$PATH`, then fall
/// back to `~/.local/bin` (where `pip install --user` / `uv tool` put it).
pub fn esptool_path() -> Option<PathBuf> {
    let names = ["esptool", "esptool.py"];

    if let Ok(path) = std::env::var("PATH") {
        for dir in std::env::split_paths(&path) {
            for name in names {
                let candidate = dir.join(name);
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    if let Ok(home) = std::env::var("HOME") {
        let local_bin = PathBuf::from(home).join(".local/bin");
        for name in names {
            let candidate = local_bin.join(name);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }

    None
}

/// True if `path` is a regular file the current user can execute.
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

/// A single guided-checklist item: whether the prerequisite is satisfied and
/// the line to show the user (a hint when unsatisfied).
pub type ChecklistItem = (bool, String);

/// Build the guided installer checklist from the current environment state.
///
/// Pure: takes the resolved facts and returns display rows. The three rows are
/// (1) firmware image, (2) esptool, (3) device state. `ready()` is true iff all
/// three are satisfied, i.e. a real flash could be attempted.
pub fn checklist(
    image: &Result<PathBuf, String>,
    esptool: Option<&Path>,
    device: DeviceState,
) -> Vec<ChecklistItem> {
    let image_row = match image {
        Ok(p) => (true, format!("firmware image: {}", p.display())),
        Err(_) => (
            false,
            "firmware image: not built — cd firmware && cargo build --release (see firmware/README.md)"
                .to_string(),
        ),
    };

    let esptool_row = match esptool {
        Some(p) => (true, format!("esptool: {}", p.display())),
        None => (
            false,
            "esptool: not found — install it (pip install esptool)".to_string(),
        ),
    };

    let device_row = match device {
        DeviceState::Bootloader => {
            (true, "device: in bootloader mode (ready to flash)".to_string())
        }
        DeviceState::NormalDevice => (
            false,
            "device: running normally — hold the boot button while plugging in USB to enter bootloader mode"
                .to_string(),
        ),
        DeviceState::Absent => (
            false,
            "device: not detected — plug in the Micro 2 (hold boot to enter bootloader mode)"
                .to_string(),
        ),
    };

    vec![image_row, esptool_row, device_row]
}

/// True iff every checklist row is satisfied (a real flash may be attempted).
pub fn ready(items: &[ChecklistItem]) -> bool {
    items.iter().all(|(ok, _)| *ok)
}

/// Flash `image` to a bootloader-mode Micro 2 via `esptool`, streaming its
/// stdout/stderr straight to the terminal.
///
/// Stops with a clear error at the first missing prerequisite (no image, no
/// esptool, device not in bootloader mode) and NEVER fabricates success. The
/// end-to-end write is only ever exercised by the user on real hardware.
pub fn flash(image: Option<&Path>, port: Option<&str>) -> Result<(), String> {
    let image = resolve_image(image)?;

    let esptool = esptool_path().ok_or_else(|| {
        "esptool not found on PATH or in ~/.local/bin. Install it first: \
         pip install esptool (or `uv tool install esptool`)."
            .to_string()
    })?;

    match classify_usb(&detect_usb()) {
        DeviceState::Bootloader => {}
        DeviceState::NormalDevice => {
            return Err(
                "the Micro 2 is connected but running normally, not in bootloader mode. \
                 It uses native USB-Serial-JTAG with no auto-reset, so you must enter \
                 download mode by hand: hold the boot button while plugging in USB, then \
                 release once it re-enumerates, and run `openmicro flash` again."
                    .to_string(),
            );
        }
        DeviceState::Absent => {
            return Err(
                "no Micro 2 detected on USB. Plug it in, and to enter bootloader mode hold \
                 the boot button while connecting (native USB-Serial-JTAG has no auto-reset)."
                    .to_string(),
            );
        }
    }

    let args = esptool_args(CHIP, port, &image);
    println!("flashing: {} {}", esptool.display(), args.join(" "));

    let status = Command::new(&esptool)
        .args(&args)
        .status()
        .map_err(|e| format!("failed to launch esptool ({}): {e}", esptool.display()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "esptool exited with {} — see its output above.",
            status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "a signal".to_string())
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn resolve_image_explicit_missing_errs() {
        let p = PathBuf::from("/definitely/not/here/openmicro-fw");
        let err = resolve_image(Some(&p)).unwrap_err();
        assert!(err.contains("not found"), "{err}");
    }

    #[test]
    fn resolve_image_explicit_existing_ok() {
        // Any existing file works; use this source file itself.
        let this = PathBuf::from(file!());
        let base = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let abs = PathBuf::from(base).join("..").join("..").join(this);
        // Fall back to a guaranteed-existing path if layout differs.
        let existing = if abs.exists() { abs } else { PathBuf::from("/etc/hostname") };
        assert!(existing.exists(), "test precondition: {} exists", existing.display());
        assert_eq!(resolve_image(Some(&existing)).unwrap(), existing);
    }

    #[test]
    fn resolve_image_none_errs_with_build_hint() {
        // No image is built in this environment, so the default lookup fails.
        let err = resolve_image(None).unwrap_err();
        assert!(err.contains("firmware/README.md"), "{err}");
        assert!(err.contains("cargo build --release"), "{err}");
    }

    #[test]
    fn classify_usb_bootloader() {
        assert_eq!(classify_usb(&[BOOTLOADER_VID_PID]), DeviceState::Bootloader);
    }

    #[test]
    fn classify_usb_normal_device() {
        assert_eq!(classify_usb(&[NORMAL_VID_PID]), DeviceState::NormalDevice);
    }

    #[test]
    fn classify_usb_absent() {
        assert_eq!(classify_usb(&[(0x1234, 0x5678)]), DeviceState::Absent);
        assert_eq!(classify_usb(&[]), DeviceState::Absent);
    }

    #[test]
    fn classify_usb_bootloader_wins_over_normal() {
        let ids = [NORMAL_VID_PID, BOOTLOADER_VID_PID];
        assert_eq!(classify_usb(&ids), DeviceState::Bootloader);
    }

    #[test]
    fn esptool_args_without_port() {
        let img = PathBuf::from("/tmp/fw.bin");
        let args = esptool_args("esp32s3", None, &img);
        assert_eq!(
            args,
            vec!["--chip", "esp32s3", "write_flash", "0x0", "/tmp/fw.bin"]
        );
    }

    #[test]
    fn esptool_args_with_port() {
        let img = PathBuf::from("/tmp/fw.bin");
        let args = esptool_args("esp32s3", Some("/dev/ttyACM0"), &img);
        assert_eq!(
            args,
            vec![
                "--chip",
                "esp32s3",
                "--port",
                "/dev/ttyACM0",
                "write_flash",
                "0x0",
                "/tmp/fw.bin"
            ]
        );
    }

    #[test]
    fn checklist_all_missing() {
        let items = checklist(
            &Err("no image".to_string()),
            None,
            DeviceState::Absent,
        );
        assert_eq!(items.len(), 3);
        assert!(!items[0].0 && !items[1].0 && !items[2].0);
        assert!(!ready(&items));
        assert!(items[0].1.contains("cargo build"));
        assert!(items[1].1.contains("install"));
        assert!(items[2].1.contains("not detected"));
    }

    #[test]
    fn checklist_normal_device_hint() {
        let items = checklist(
            &Ok(PathBuf::from("/tmp/fw.bin")),
            Some(Path::new("/usr/bin/esptool")),
            DeviceState::NormalDevice,
        );
        assert!(items[0].0, "image ok");
        assert!(items[1].0, "esptool ok");
        assert!(!items[2].0, "device not ready");
        assert!(items[2].1.contains("boot button"));
        assert!(!ready(&items));
    }

    #[test]
    fn checklist_all_ready() {
        let items = checklist(
            &Ok(PathBuf::from("/tmp/fw.bin")),
            Some(Path::new("/usr/bin/esptool")),
            DeviceState::Bootloader,
        );
        assert!(ready(&items));
        assert!(items[2].1.contains("ready to flash"));
    }

    #[test]
    fn esptool_path_finds_installed_esptool() {
        // esptool is installed at ~/.local/bin in this environment.
        assert!(
            esptool_path().is_some(),
            "expected to find esptool on PATH or in ~/.local/bin"
        );
    }
}

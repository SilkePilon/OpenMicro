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

use std::path::{Path, PathBuf};
use std::process::Command;

/// Default flashable-image path produced by `cargo build --release` in
/// `firmware/`, relative to the repo root (or the current directory).
pub const DEFAULT_IMAGE_REL: &str =
    "firmware/target/xtensa-esp32s3-none-elf/release/openmicro-fw";

/// esptool/espflash chip argument for this board.
pub const CHIP: &str = "esp32s3";

/// Where a full stock-firmware backup is written by [`backup`] and read back by
/// [`restore`]. Keeping the original image is what makes going back to the
/// stock firmware possible at all.
pub const BACKUP_REL: &str = ".local/share/openmicro/stock-firmware.bin";

/// Flash size dumped by [`backup`]: 4 MiB, the Micro 2's documented size.
pub const FLASH_SIZE: &str = "0x400000";

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
        if p.is_dir() {
            return Err(format!(
                "firmware image is not a file: {} is a directory (check the --image path)",
                p.display()
            ));
        }
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
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    // A firmware image downloaded by `openmicro firmware download` (or the TUI
    // setup wizard) is equally flashable, and is the only source available when
    // running from an installed binary rather than a source checkout.
    let cached = crate::firmware::cache_image();
    if cached.is_file() {
        return Ok(cached);
    }

    Err(format!(
        "no firmware image found (looked for {DEFAULT_IMAGE_REL} and {}). \
         Get one first: `openmicro firmware build` (needs the Xtensa toolchain — see \
         firmware/README.md) or `openmicro firmware download`.",
        cached.display()
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
///
/// Delegates the product-id table to [`crate::wldevice`], which knows every id
/// the hardware ships under (two Creator Micro 2 revisions plus the Codex
/// Micro) rather than only the one this module originally hardcoded.
pub fn classify_usb(ids: &[(u16, u16)]) -> DeviceState {
    match crate::wldevice::classify(ids) {
        crate::wldevice::UsbMode::Bootloader => DeviceState::Bootloader,
        crate::wldevice::UsbMode::App(_) => DeviceState::NormalDevice,
        crate::wldevice::UsbMode::Absent => DeviceState::Absent,
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
///
/// The reset options are not incidental. `--before no-reset` is required
/// because the device is already in download mode and esptool's usual reset
/// would knock it out; `--after watchdog-reset` is the only reset that works on
/// this board, which has native USB-Serial-JTAG and therefore no DTR/RTS lines
/// for the classic auto-reset. See [`crate::wldevice`].
pub fn esptool_args(chip: &str, port: Option<&str>, image: &Path) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), chip.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("--before".to_string());
    args.push("no-reset".to_string());
    args.push("--after".to_string());
    args.push("watchdog-reset".to_string());
    args.push("write_flash".to_string());
    args.push("0x0".to_string());
    args.push(image.display().to_string());
    args
}

/// Locate the first of `names` that is an executable: honor `$PATH`, then fall
/// back to `~/.local/bin` and `~/.cargo/bin` (where `pip install --user`,
/// `uv tool install` and `cargo install` put things without always being on the
/// PATH of a non-login shell).
pub fn which(names: &[&str]) -> Option<PathBuf> {
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
        let home = PathBuf::from(home);
        for extra in [".local/bin", ".cargo/bin"] {
            for name in names {
                let candidate = home.join(extra).join(name);
                if is_executable(&candidate) {
                    return Some(candidate);
                }
            }
        }
    }

    None
}

/// Locate an `esptool` / `esptool.py` executable.
pub fn esptool_path() -> Option<PathBuf> {
    which(&["esptool", "esptool.py"])
}

/// Locate `espflash`, the Rust ESP flashing tool (`cargo install espflash`).
pub fn espflash_path() -> Option<PathBuf> {
    which(&["espflash"])
}

/// True if `path` is a regular file the current user can execute.
pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

/// True if `path` starts with the ELF magic (`\x7fELF`).
///
/// This is what distinguishes the two kinds of image we can be handed: the
/// `firmware/` build output is an **ELF**, while a downloaded release asset is a
/// **merged flash image**. They need different tools, so guessing from the file
/// extension (the build output has none) is not good enough.
pub fn is_elf(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    matches!(f.read_exact(&mut magic), Ok(())) && magic == *b"\x7fELF"
}

/// The tool that will perform the write, chosen from the image's format.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flasher {
    /// `espflash`: takes the firmware **ELF** directly and derives the
    /// bootloader, partition table and app offsets itself.
    Espflash(PathBuf),
    /// `esptool`: writes a **merged binary** (bootloader + partition table +
    /// app already combined) at offset `0x0`.
    Esptool(PathBuf),
}

impl Flasher {
    /// Path of the executable to run.
    pub fn program(&self) -> &Path {
        match self {
            Flasher::Espflash(p) | Flasher::Esptool(p) => p,
        }
    }

    /// Argument vector for writing `image` to the device.
    pub fn args(&self, chip: &str, port: Option<&str>, image: &Path) -> Vec<String> {
        match self {
            Flasher::Espflash(_) => espflash_args(chip, port, image),
            Flasher::Esptool(_) => esptool_args(chip, port, image),
        }
    }
}

/// Pick the flashing tool for `image`, or explain what to install.
///
/// An ELF requires `espflash` — handing a bare ELF to `esptool write_flash`
/// would write a file the chip cannot boot, so we refuse rather than produce a
/// bricked device. A merged `.bin` works with either tool; `esptool` is
/// preferred there because that is the documented layout.
pub fn pick_flasher(image: &Path) -> Result<Flasher, String> {
    if is_elf(image) {
        return espflash_path().map(Flasher::Espflash).ok_or_else(|| {
            "the firmware image is an ELF (a from-source build), which needs `espflash` to \
             derive the bootloader and partition table. Install it: cargo install espflash."
                .to_string()
        });
    }
    if let Some(p) = esptool_path() {
        return Ok(Flasher::Esptool(p));
    }
    if let Some(p) = espflash_path() {
        return Ok(Flasher::Espflash(p));
    }
    Err("no flashing tool found — install one: `pip install esptool` or \
         `cargo install espflash`."
        .to_string())
}

/// Build the `espflash` argument vector to flash an ELF image.
pub fn espflash_args(chip: &str, port: Option<&str>, image: &Path) -> Vec<String> {
    let mut args = vec!["flash".to_string(), "--chip".to_string(), chip.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    // Do not drop into the serial monitor: the TUI/CLI owns the terminal and a
    // monitor would never return.
    args.push("--non-interactive".to_string());
    args.push(image.display().to_string());
    args
}

/// Build the `esptool` argument vector to read the whole flash into `dest` —
/// the stock-firmware backup that makes a later [`restore`] possible.
pub fn backup_args(chip: &str, port: Option<&str>, dest: &Path) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), chip.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("read_flash".to_string());
    args.push("0x0".to_string());
    args.push(FLASH_SIZE.to_string());
    args.push(dest.display().to_string());
    args
}

/// Default path of the stock-firmware backup.
pub fn backup_path() -> PathBuf {
    crate::agents::home().join(BACKUP_REL)
}

/// Dump the device's entire flash to `dest` (default [`backup_path`]) so the
/// original firmware can be put back later.
///
/// Requires `esptool` and a device in bootloader mode, exactly like flashing.
/// Returns the captured output lines.
pub fn backup(dest: Option<&Path>, port: Option<&str>) -> Result<(PathBuf, Vec<String>), String> {
    let esptool = esptool_path().ok_or_else(|| {
        "reading the stock firmware needs esptool. Install it: pip install esptool.".to_string()
    })?;
    require_bootloader()?;

    let dest = dest.map(|p| p.to_path_buf()).unwrap_or_else(backup_path);
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }

    let args = backup_args(CHIP, port, &dest);
    let output = Command::new(&esptool)
        .args(&args)
        .output()
        .map_err(|e| format!("failed to launch esptool ({}): {e}", esptool.display()))?;
    let mut lines = combine_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        lines.push(format!("esptool exited with {}.", exit_desc(output.status.code())));
        return Err(lines.join("\n"));
    }
    lines.push(format!("stock firmware saved to {}", dest.display()));
    Ok((dest, lines))
}

/// Write a previously saved stock-firmware backup back to the device.
///
/// This is the "undo" for installing custom firmware. It is *not* a supported
/// use of the hardware by its vendor — it only works because [`backup`] kept a
/// byte-for-byte copy of the original flash — so it refuses outright when no
/// backup exists rather than pretending.
pub fn restore(image: Option<&Path>, port: Option<&str>) -> Result<Vec<String>, String> {
    let path = image.map(|p| p.to_path_buf()).unwrap_or_else(backup_path);
    if !path.is_file() {
        return Err(format!(
            "no stock-firmware backup at {} — a restore is only possible from a backup taken \
             BEFORE flashing custom firmware (`openmicro backup`).",
            path.display()
        ));
    }
    let esptool = esptool_path().ok_or_else(|| {
        "restoring needs esptool. Install it: pip install esptool.".to_string()
    })?;
    require_bootloader()?;

    let args = esptool_args(CHIP, port, &path);
    let output = Command::new(&esptool)
        .args(&args)
        .output()
        .map_err(|e| format!("failed to launch esptool ({}): {e}", esptool.display()))?;
    let mut lines = combine_output(&output.stdout, &output.stderr);
    if !output.status.success() {
        lines.push(format!("esptool exited with {}.", exit_desc(output.status.code())));
        return Err(lines.join("\n"));
    }
    lines.push(format!("restored {} — reset the device.", path.display()));
    Ok(lines)
}

/// Merge a child process's stdout and stderr into display lines.
pub fn combine_output(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .chain(String::from_utf8_lossy(stderr).lines())
        .map(|l| l.to_string())
        .collect()
}

/// Error out unless a Micro 2 is currently in ROM bootloader mode.
fn require_bootloader() -> Result<(), String> {
    match classify_usb(&detect_usb()) {
        DeviceState::Bootloader => Ok(()),
        DeviceState::NormalDevice => Err(BOOTLOADER_HINT_CONNECTED.to_string()),
        DeviceState::Absent => Err(BOOTLOADER_HINT_ABSENT.to_string()),
    }
}

/// Guidance shown when the device is plugged in but running its firmware.
pub const BOOTLOADER_HINT_CONNECTED: &str =
    "the Micro 2 is connected but running normally, not in bootloader mode. It uses native \
     USB-Serial-JTAG with no auto-reset, so you must enter download mode by hand: hold the \
     boot/power button while plugging in USB, then release once it re-enumerates.";

/// Guidance shown when no Micro 2 is on the bus at all.
pub const BOOTLOADER_HINT_ABSENT: &str =
    "no Micro 2 detected on USB. Plug it in with a data-capable USB cable, and to enter \
     bootloader mode hold the boot/power button while connecting (native USB-Serial-JTAG has \
     no auto-reset).";

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
    flasher: &Result<Flasher, String>,
    device: DeviceState,
) -> Vec<ChecklistItem> {
    let image_row = match image {
        Ok(p) => (true, format!("firmware image: {}", p.display())),
        Err(_) => (
            false,
            "firmware image: none yet — `openmicro firmware build` or `openmicro firmware download`"
                .to_string(),
        ),
    };

    let flasher_row = match flasher {
        Ok(f) => (true, format!("flash tool: {}", f.program().display())),
        Err(e) => (false, format!("flash tool: {e}")),
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

    vec![image_row, flasher_row, device_row]
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
    let (tool, args) = prepare(image, port)?;
    println!("flashing: {} {}", tool.display(), args.join(" "));

    let status = Command::new(&tool)
        .args(&args)
        .status()
        .map_err(|e| format!("failed to launch {}: {e}", tool.display()))?;

    if status.success() {
        Ok(())
    } else {
        Err(format!(
            "the flash tool exited with {} — see its output above.",
            exit_desc(status.code())
        ))
    }
}

/// Like [`flash`], but captures esptool's combined stdout/stderr and returns it
/// as lines (for the TUI's scrollable output area) instead of streaming to the
/// terminal. Same prerequisite checks; same honesty contract.
pub fn flash_capture(image: Option<&Path>, port: Option<&str>) -> Result<Vec<String>, String> {
    let (tool, args) = prepare(image, port)?;

    let output = Command::new(&tool)
        .args(&args)
        .output()
        .map_err(|e| format!("failed to launch {}: {e}", tool.display()))?;

    let mut lines = combine_output(&output.stdout, &output.stderr);

    if output.status.success() {
        Ok(lines)
    } else {
        lines.push(format!(
            "the flash tool exited with {}.",
            exit_desc(output.status.code())
        ));
        Err(lines.join("\n"))
    }
}

/// Resolve and validate all flash prerequisites (image, esptool, bootloader-mode
/// device) and return the esptool executable plus its argv. Returns an
/// actionable `Err` at the first missing prerequisite; shared by [`flash`] and
/// [`flash_capture`] so both stay honest and consistent.
fn prepare(image: Option<&Path>, port: Option<&str>) -> Result<(PathBuf, Vec<String>), String> {
    let image = resolve_image(image)?;
    let flasher = pick_flasher(&image)?;
    require_bootloader()?;
    Ok((flasher.program().to_path_buf(), flasher.args(CHIP, port, &image)))
}

/// Human-readable description of a process exit code (or a signal if none).
pub fn exit_desc(code: Option<i32>) -> String {
    code.map(|c| c.to_string())
        .unwrap_or_else(|| "a signal".to_string())
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
        // Any existing file works. `CARGO_MANIFEST_DIR`/Cargo.toml is portable
        // across every machine that can even run this test (cargo guarantees
        // it), unlike the previous fallback to `/etc/hostname` (Linux-only,
        // and not guaranteed to exist on minimal/container images).
        let base = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let existing = PathBuf::from(base).join("Cargo.toml");
        assert!(existing.exists(), "test precondition: {} exists", existing.display());
        assert_eq!(resolve_image(Some(&existing)).unwrap(), existing);
    }

    #[test]
    fn resolve_image_explicit_directory_errs() {
        // A directory technically "exists" but is never a valid flashable
        // image; `--image <a directory>` must fail with a clear message
        // rather than being handed to esptool as-is.
        let base = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let dir = PathBuf::from(base); // the crate root: exists, is a directory
        assert!(dir.is_dir(), "test precondition: {} is a directory", dir.display());
        let err = resolve_image(Some(&dir)).unwrap_err();
        assert!(err.contains("not a file") || err.contains("directory"), "{err}");
    }

    #[test]
    fn resolve_image_none_errs_with_build_hint() {
        // No image is built in this environment, so the default lookup fails.
        let err = resolve_image(None).unwrap_err();
        assert!(err.contains("firmware/README.md"), "{err}");
        // Both ways to get an image are offered, and the download cache is
        // named so it is obvious where a fetched image would have landed.
        assert!(err.contains("openmicro firmware build"), "{err}");
        assert!(err.contains("openmicro firmware download"), "{err}");
        assert!(err.contains(crate::firmware::CACHE_REL), "{err}");
    }

    #[test]
    fn classify_usb_bootloader() {
        assert_eq!(classify_usb(&[(crate::wldevice::WL_VID, crate::wldevice::BOOTLOADER_PID)]), DeviceState::Bootloader);
    }

    #[test]
    fn classify_usb_normal_device() {
        assert_eq!(classify_usb(&[(crate::wldevice::WL_VID, crate::wldevice::APP_PIDS[1])]), DeviceState::NormalDevice);
    }

    #[test]
    fn classify_usb_absent() {
        assert_eq!(classify_usb(&[(0x1234, 0x5678)]), DeviceState::Absent);
        assert_eq!(classify_usb(&[]), DeviceState::Absent);
    }

    #[test]
    fn classify_usb_bootloader_wins_over_normal() {
        let ids = [
            (crate::wldevice::WL_VID, crate::wldevice::APP_PIDS[1]),
            (crate::wldevice::WL_VID, crate::wldevice::BOOTLOADER_PID),
        ];
        assert_eq!(classify_usb(&ids), DeviceState::Bootloader);
    }

    #[test]
    fn esptool_args_without_port() {
        let img = PathBuf::from("/tmp/fw.bin");
        let args = esptool_args("esp32s3", None, &img);
        assert_eq!(
            args,
            vec![
                "--chip",
                "esp32s3",
                "--before",
                "no-reset",
                "--after",
                "watchdog-reset",
                "write_flash",
                "0x0",
                "/tmp/fw.bin"
            ]
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
                "--before",
                "no-reset",
                "--after",
                "watchdog-reset",
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
            &Err("no flash tool".to_string()),
            DeviceState::Absent,
        );
        assert_eq!(items.len(), 3);
        assert!(!items[0].0 && !items[1].0 && !items[2].0);
        assert!(!ready(&items));
        assert!(items[0].1.contains("firmware build") || items[0].1.contains("download"));
        assert!(items[1].1.contains("no flash tool"));
        assert!(items[2].1.contains("not detected"));
    }

    #[test]
    fn checklist_normal_device_hint() {
        let items = checklist(
            &Ok(PathBuf::from("/tmp/fw.bin")),
            &Ok(Flasher::Esptool(PathBuf::from("/usr/bin/esptool"))),
            DeviceState::NormalDevice,
        );
        assert!(items[0].0, "image ok");
        assert!(items[1].0, "flash tool ok");
        assert!(!items[2].0, "device not ready");
        assert!(items[2].1.contains("boot button"));
        assert!(!ready(&items));
    }

    #[test]
    fn checklist_all_ready() {
        let items = checklist(
            &Ok(PathBuf::from("/tmp/fw.bin")),
            &Ok(Flasher::Esptool(PathBuf::from("/usr/bin/esptool"))),
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

    #[test]
    fn which_finds_a_ubiquitous_binary_and_misses_a_fake_one() {
        assert!(which(&["sh"]).is_some(), "sh must be findable on PATH");
        assert!(which(&["definitely-not-a-real-binary-xyz"]).is_none());
    }

    #[test]
    fn is_elf_detects_a_real_elf_and_rejects_other_files() {
        // The test binary itself is an ELF on Linux.
        let me = std::env::current_exe().unwrap();
        assert!(is_elf(&me), "the test executable should be an ELF");
        let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("Cargo.toml");
        assert!(!is_elf(&manifest), "a TOML file is not an ELF");
        assert!(!is_elf(Path::new("/nope/missing")), "a missing file is not an ELF");
    }

    #[test]
    fn espflash_args_flash_an_elf_without_a_monitor() {
        let img = PathBuf::from("/tmp/openmicro-fw");
        let args = espflash_args("esp32s3", Some("/dev/ttyACM0"), &img);
        assert_eq!(args[0], "flash");
        assert!(args.contains(&"--non-interactive".to_string()), "must not open a monitor");
        assert!(args.contains(&"/dev/ttyACM0".to_string()));
        assert_eq!(args.last().unwrap(), "/tmp/openmicro-fw");
    }

    #[test]
    fn flasher_args_dispatch_on_tool() {
        let img = PathBuf::from("/tmp/fw.bin");
        let esptool = Flasher::Esptool(PathBuf::from("/usr/bin/esptool"));
        assert_eq!(esptool.args("esp32s3", None, &img), esptool_args("esp32s3", None, &img));
        assert_eq!(esptool.program(), Path::new("/usr/bin/esptool"));

        let espflash = Flasher::Espflash(PathBuf::from("/usr/bin/espflash"));
        assert_eq!(espflash.args("esp32s3", None, &img), espflash_args("esp32s3", None, &img));
    }

    #[test]
    fn pick_flasher_refuses_an_elf_without_espflash() {
        // An ELF handed to `esptool write_flash 0x0` would write something the
        // chip cannot boot, so with no espflash installed we must refuse.
        if espflash_path().is_some() {
            return; // espflash present here: the refusal path is not reachable
        }
        let me = std::env::current_exe().unwrap();
        let err = pick_flasher(&me).unwrap_err();
        assert!(err.contains("espflash"), "{err}");
    }

    #[test]
    fn pick_flasher_uses_esptool_for_a_merged_bin() {
        let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("Cargo.toml"); // any non-ELF file
        match pick_flasher(&manifest) {
            Ok(Flasher::Esptool(_)) => {}
            other => panic!("expected esptool for a non-ELF image, got {other:?}"),
        }
    }

    #[test]
    fn backup_args_read_the_whole_flash() {
        let args = backup_args("esp32s3", None, Path::new("/tmp/stock.bin"));
        assert_eq!(
            args,
            vec!["--chip", "esp32s3", "read_flash", "0x0", FLASH_SIZE, "/tmp/stock.bin"]
        );
    }

    #[test]
    fn backup_args_include_the_port_when_given() {
        let args = backup_args("esp32s3", Some("/dev/ttyACM0"), Path::new("/tmp/s.bin"));
        assert!(args.windows(2).any(|w| w == ["--port", "/dev/ttyACM0"]));
    }

    #[test]
    fn restore_without_a_backup_refuses() {
        let err = restore(Some(Path::new("/definitely/not/here/stock.bin")), None).unwrap_err();
        assert!(err.contains("no stock-firmware backup"), "{err}");
        assert!(err.contains("BEFORE"), "must explain when a backup had to be taken: {err}");
    }

    #[test]
    fn backup_path_is_under_the_users_data_dir() {
        assert!(backup_path().ends_with(BACKUP_REL));
    }
}

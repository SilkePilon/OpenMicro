use std::path::{Path, PathBuf};
use std::process::Command;

pub const DEFAULT_IMAGE_REL: &str =
    "firmware/target/xtensa-esp32s3-none-elf/release/openmicro-fw";

pub const CHIP: &str = "esp32s3";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DeviceState {
    Bootloader,
    NormalDevice,
    Absent,
}

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

    resolve_default(&candidate_roots(), &crate::firmware::cache_image())
}

fn resolve_default(roots: &[PathBuf], cached: &Path) -> Result<PathBuf, String> {
    for base in roots {
        let candidate = base.join(DEFAULT_IMAGE_REL);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }

    if cached.is_file() {
        return Ok(cached.to_path_buf());
    }

    Err(format!(
        "no firmware image found (looked for {DEFAULT_IMAGE_REL} and {}). \
         Get one first: build it from source, or download a published version.",
        cached.display()
    ))
}

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

pub fn classify_usb(ids: &[(u16, u16)]) -> DeviceState {
    match crate::wldevice::classify(ids) {
        crate::wldevice::UsbMode::Bootloader => DeviceState::Bootloader,
        crate::wldevice::UsbMode::App(_) => DeviceState::NormalDevice,
        crate::wldevice::UsbMode::Absent => DeviceState::Absent,
    }
}

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

fn read_hex_u16(path: &Path) -> Option<u16> {
    let s = std::fs::read_to_string(path).ok()?;
    u16::from_str_radix(s.trim(), 16).ok()
}

pub fn esptool_args(chip: &str, port: Option<&str>, image: &Path, major: Option<u32>) -> Vec<String> {
    let mut args = vec!["--chip".to_string(), chip.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("--before".to_string());
    args.push("usb-reset".to_string());
    args.push("--after".to_string());
    args.push("no-reset".to_string());
    args.push(subcommand("write_flash", major));
    args.push("0x0".to_string());
    args.push(image.display().to_string());
    args
}

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

pub fn esptool_major(path: &Path) -> Option<u32> {
    let output = Command::new(path).arg("version").output().ok()?;
    let text = String::from_utf8_lossy(&output.stdout);
    let version = text.split_whitespace().find(|w| w.starts_with('v') || w.starts_with(char::is_numeric))?;
    version.trim_start_matches('v').split('.').next()?.parse().ok()
}

pub fn subcommand(name: &str, major: Option<u32>) -> String {
    match major {
        Some(m) if m >= 5 => name.replace('_', "-"),
        _ => name.to_string(),
    }
}

pub fn esptool_path() -> Option<PathBuf> {
    which(&["esptool", "esptool.py"])
}

pub fn espflash_path() -> Option<PathBuf> {
    which(&["espflash"])
}

pub fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(path) {
        Ok(m) => m.is_file() && (m.permissions().mode() & 0o111 != 0),
        Err(_) => false,
    }
}

pub fn is_elf(path: &Path) -> bool {
    use std::io::Read;
    let Ok(mut f) = std::fs::File::open(path) else {
        return false;
    };
    let mut magic = [0u8; 4];
    matches!(f.read_exact(&mut magic), Ok(())) && magic == *b"\x7fELF"
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Flasher {
    Espflash(PathBuf),
    Esptool(PathBuf),
}

impl Flasher {
    pub fn program(&self) -> &Path {
        match self {
            Flasher::Espflash(p) | Flasher::Esptool(p) => p,
        }
    }

    pub fn args(&self, chip: &str, port: Option<&str>, image: &Path) -> Vec<String> {
        match self {
            Flasher::Espflash(_) => espflash_args(chip, port, image),
            Flasher::Esptool(p) => esptool_args(chip, port, image, esptool_major(p)),
        }
    }
}

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

pub fn espflash_args(chip: &str, port: Option<&str>, image: &Path) -> Vec<String> {
    let mut args = vec!["flash".to_string(), "--chip".to_string(), chip.to_string()];
    if let Some(p) = port {
        args.push("--port".to_string());
        args.push(p.to_string());
    }
    args.push("--non-interactive".to_string());
    args.push(image.display().to_string());
    args
}

pub fn restore(image: &Path, port: Option<&str>) -> Result<Vec<String>, String> {
    let path = image.to_path_buf();
    if !path.is_file() {
        return Err(format!(
            "no firmware image at {}. Download one of Work Louder's published releases.",
            path.display()
        ));
    }
    let esptool = esptool_path().ok_or_else(|| {
        "restoring needs esptool. Install it: pip install esptool.".to_string()
    })?;
    require_bootloader()?;

    let args = esptool_args(CHIP, port, &path, esptool_major(&esptool));
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

pub fn combine_output(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .chain(String::from_utf8_lossy(stderr).lines())
        .map(|l| l.to_string())
        .collect()
}

pub fn port_contention() -> Option<String> {
    if !which(&["systemctl"]).is_some_and(|_| true) {
        return None;
    }
    let active = Command::new("systemctl")
        .args(["is-active", "ModemManager"])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim() == "active")
        .unwrap_or(false);
    if !active {
        return None;
    }
    Some(
        "ModemManager is running. It opens every new /dev/ttyACM* device, which \
         interrupts flashing partway through.\n\n\
         Stop it for now:\n    sudo systemctl stop ModemManager\n\n\
         To exclude the device permanently, see docs/troubleshooting.md."
            .to_string(),
    )
}

fn require_bootloader() -> Result<(), String> {
    match classify_usb(&detect_usb()) {
        DeviceState::Bootloader => Ok(()),
        DeviceState::NormalDevice => Err(BOOTLOADER_HINT_CONNECTED.to_string()),
        DeviceState::Absent => Err(BOOTLOADER_HINT_ABSENT.to_string()),
    }
}

pub const BOOTLOADER_HINT_CONNECTED: &str =
    "the Micro 2 is connected but running normally, not in bootloader mode.";

pub const BOOTLOADER_HINT_ABSENT: &str =
    "no Micro 2 detected on USB. Connect it with a data-capable cable.";

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

fn prepare(image: Option<&Path>, port: Option<&str>) -> Result<(PathBuf, Vec<String>), String> {
    let image = resolve_image(image)?;
    let flasher = pick_flasher(&image)?;
    require_bootloader()?;
    Ok((flasher.program().to_path_buf(), flasher.args(CHIP, port, &image)))
}

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
        let base = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let existing = PathBuf::from(base).join("Cargo.toml");
        assert!(existing.exists(), "test precondition: {} exists", existing.display());
        assert_eq!(resolve_image(Some(&existing)).unwrap(), existing);
    }

    #[test]
    fn resolve_image_explicit_directory_errs() {
        let base = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        let dir = PathBuf::from(base);
        assert!(dir.is_dir(), "test precondition: {} is a directory", dir.display());
        let err = resolve_image(Some(&dir)).unwrap_err();
        assert!(err.contains("not a file") || err.contains("directory"), "{err}");
    }

    #[test]
    fn resolve_image_none_errs_with_build_hint() {
        let err = resolve_default(&[], Path::new("/nonexistent/openmicro-fw.bin")).unwrap_err();
        assert!(err.contains(DEFAULT_IMAGE_REL), "{err}");
        assert!(err.contains("/nonexistent/openmicro-fw.bin"), "names the cache: {err}");
        assert!(err.contains("build it from source"), "{err}");
        assert!(err.contains("download"), "{err}");
    }

    #[test]
    fn resolve_default_prefers_a_source_build_over_the_download_cache() {
        let dir = std::env::temp_dir().join(format!("openmicro-resolve-{}", std::process::id()));
        let built = dir.join(DEFAULT_IMAGE_REL);
        std::fs::create_dir_all(built.parent().unwrap()).unwrap();
        std::fs::write(&built, b"elf").unwrap();
        let cached = dir.join("cached.bin");
        std::fs::write(&cached, b"bin").unwrap();

        let picked = resolve_default(std::slice::from_ref(&dir), &cached).unwrap();

        let _ = std::fs::remove_dir_all(&dir);
        assert_eq!(picked, built, "a local build wins over a stale download");
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
        let args = esptool_args("esp32s3", None, &img, Some(4));
        assert_eq!(
            args,
            vec![
                "--chip",
                "esp32s3",
                "--before",
                "usb-reset",
                "--after",
                "no-reset",
                "write_flash",
                "0x0",
                "/tmp/fw.bin"
            ]
        );
    }

    #[test]
    fn esptool_args_with_port() {
        let img = PathBuf::from("/tmp/fw.bin");
        let args = esptool_args("esp32s3", Some("/dev/ttyACM0"), &img, Some(4));
        assert_eq!(
            args,
            vec![
                "--chip",
                "esp32s3",
                "--port",
                "/dev/ttyACM0",
                "--before",
                "usb-reset",
                "--after",
                "no-reset",
                "write_flash",
                "0x0",
                "/tmp/fw.bin"
            ]
        );
    }

    #[test]
    fn esptool_path_finds_installed_esptool() {
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
        assert_eq!(
            esptool.args("esp32s3", None, &img),
            esptool_args("esp32s3", None, &img, esptool_major(Path::new("/usr/bin/esptool")))
        );
        assert_eq!(esptool.program(), Path::new("/usr/bin/esptool"));

        let espflash = Flasher::Espflash(PathBuf::from("/usr/bin/espflash"));
        assert_eq!(espflash.args("esp32s3", None, &img), espflash_args("esp32s3", None, &img));
    }

    #[test]
    fn pick_flasher_refuses_an_elf_without_espflash() {
        if espflash_path().is_some() {
            return;
        }
        let me = std::env::current_exe().unwrap();
        let err = pick_flasher(&me).unwrap_err();
        assert!(err.contains("espflash"), "{err}");
    }

    #[test]
    fn pick_flasher_uses_esptool_for_a_merged_bin() {
        let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap())
            .join("Cargo.toml");
        match pick_flasher(&manifest) {
            Ok(Flasher::Esptool(_)) => {}
            other => panic!("expected esptool for a non-ELF image, got {other:?}"),
        }
    }

    #[test]
    fn restoring_a_missing_image_refuses() {
        let err = restore(Path::new("/definitely/not/here/stock.bin"), None).unwrap_err();
        assert!(err.contains("no firmware image"), "{err}");
        assert!(err.contains("Work Louder"), "points at the vendor releases: {err}");
    }

    #[test]
    fn subcommand_spelling_follows_the_esptool_major_version() {
        assert_eq!(subcommand("read_flash", Some(5)), "read-flash");
        assert_eq!(subcommand("write_flash", Some(5)), "write-flash");
        assert_eq!(subcommand("read_flash", Some(4)), "read_flash");
        assert_eq!(subcommand("read_flash", None), "read_flash");
    }

    #[test]
    fn esptool_major_reads_the_installed_version() {
        let path = esptool_path().expect("esptool present");
        assert!(esptool_major(&path).is_some(), "could not parse esptool version");
    }
}

//! Getting a flashable OpenMicro firmware image: **build it** from the
//! `firmware/` crate, or **download** a prebuilt release.
//!
//! Building needs the Xtensa Rust toolchain (`espup`), which is a large
//! one-time install and is not present on most machines; downloading needs a
//! published release. Neither is guaranteed, so every entry point here reports
//! precisely which of the two is available and why the other is not, and never
//! claims to have produced an image it did not produce.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Where a downloaded firmware image is cached.
pub const CACHE_REL: &str = ".cache/openmicro/firmware/openmicro-fw.bin";

/// Release asset fetched by [`download`] when `$OPENMICRO_FIRMWARE_URL` is
/// unset. Points at the "latest release" redirect so it keeps working across
/// versions; until a release exists it 404s, which surfaces as a plain
/// "download failed" rather than a silent bad image.
pub const DEFAULT_FIRMWARE_URL: &str =
    "https://github.com/SilkePilon/OpenMicro/releases/latest/download/openmicro-fw.bin";

/// Environment variable that overrides [`DEFAULT_FIRMWARE_URL`] (a fork, a
/// self-hosted build, or a `file://` path for testing).
pub const FIRMWARE_URL_ENV: &str = "OPENMICRO_FIRMWARE_URL";

/// Whether the Xtensa toolchain needed to build the firmware is usable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Toolchain {
    /// `~/export-esp.sh` exists; sourcing it puts the `esp` toolchain on PATH.
    Ready(PathBuf),
    /// `espup` is installed but has not been run (`espup install`).
    NeedsInstall,
    /// Neither `espup` nor its export script is present.
    Missing,
}

impl Toolchain {
    /// One-line status for the wizard.
    pub fn describe(&self) -> String {
        match self {
            Toolchain::Ready(p) => format!("Xtensa toolchain ready ({})", p.display()),
            Toolchain::NeedsInstall => {
                "espup found but not installed — run: espup install".to_string()
            }
            Toolchain::Missing => {
                "Xtensa toolchain missing — install it: cargo install espup && espup install"
                    .to_string()
            }
        }
    }

    pub fn is_ready(&self) -> bool {
        matches!(self, Toolchain::Ready(_))
    }
}

/// Locate the `firmware/` crate directory: the current directory or the nearest
/// ancestor that contains one.
pub fn firmware_dir() -> Option<PathBuf> {
    let cwd = std::env::current_dir().ok()?;
    let mut dir: Option<&Path> = Some(cwd.as_path());
    while let Some(d) = dir {
        let candidate = d.join("firmware");
        if candidate.join("Cargo.toml").is_file() {
            return Some(candidate);
        }
        dir = d.parent();
    }
    None
}

/// Detect the Xtensa toolchain: the `espup` export script, else `espup` itself.
pub fn toolchain() -> Toolchain {
    if let Ok(home) = std::env::var("HOME") {
        let export = PathBuf::from(home).join("export-esp.sh");
        if export.is_file() {
            return Toolchain::Ready(export);
        }
    }
    if crate::flash::which(&["espup"]).is_some() {
        Toolchain::NeedsInstall
    } else {
        Toolchain::Missing
    }
}

/// The shell command that builds the firmware.
///
/// Pure so the exact invocation is unit-tested: the Xtensa toolchain only
/// exists inside the environment `export-esp.sh` sets up, so the build has to
/// run under a shell that sources it first.
pub fn build_script(dir: &Path, export: &Path) -> String {
    format!(
        "set -e; . {}; cd {}; cargo build --release",
        shell_quote(&export.display().to_string()),
        shell_quote(&dir.display().to_string())
    )
}

/// Single-quote a string for `sh -c`, escaping embedded quotes.
fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', r"'\''"))
}

/// Cached download destination (`~/.cache/openmicro/firmware/openmicro-fw.bin`).
pub fn cache_image() -> PathBuf {
    crate::agents::home().join(CACHE_REL)
}

/// The URL [`download`] fetches: `$OPENMICRO_FIRMWARE_URL` if set and non-empty,
/// else [`DEFAULT_FIRMWARE_URL`].
pub fn firmware_url() -> String {
    std::env::var(FIRMWARE_URL_ENV)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_FIRMWARE_URL.to_string())
}

/// Build the firmware from source. Returns the built image path plus the
/// build's captured output lines.
///
/// Fails loudly (with cargo's own output) when the toolchain is absent, the
/// `firmware/` directory cannot be found, or the build does not succeed. The
/// embedded crate has never been compiled on a machine without the Xtensa
/// toolchain, so a first build legitimately may need source fixes — those show
/// up here as real compiler errors rather than a fabricated success.
pub fn build() -> Result<(PathBuf, Vec<String>), String> {
    let Toolchain::Ready(export) = toolchain() else {
        return Err(toolchain().describe());
    };
    let dir = firmware_dir().ok_or_else(|| {
        "no firmware/ directory found from the current directory upwards — run this from \
         an OpenMicro checkout, or use the download option instead."
            .to_string()
    })?;

    let script = build_script(&dir, &export);
    let output = Command::new("sh")
        .arg("-c")
        .arg(&script)
        .output()
        .map_err(|e| format!("cannot run the firmware build: {e}"))?;

    let mut lines = capture_lines(&output.stdout, &output.stderr);
    if !output.status.success() {
        lines.push(format!(
            "firmware build failed ({}).",
            crate::flash::exit_desc(output.status.code())
        ));
        return Err(lines.join("\n"));
    }

    // Trust the filesystem, not the exit code: confirm the artifact exists.
    let image = crate::flash::resolve_image(None).map_err(|e| {
        format!("build reported success but no image was found: {e}")
    })?;
    lines.push(format!("built {}", image.display()));
    Ok((image, lines))
}

/// Download a prebuilt firmware image into the cache directory.
///
/// Uses `curl` (present on every machine that can install this project) with
/// `--fail`, so an HTML 404 page is never mistaken for firmware. The file is
/// written to a temp path and renamed only after a successful transfer, so a
/// half-downloaded image can never be flashed.
pub fn download() -> Result<(PathBuf, Vec<String>), String> {
    let curl = crate::flash::which(&["curl"])
        .ok_or_else(|| "curl not found — install curl, or build the firmware instead.".to_string())?;
    let url = firmware_url();
    let dest = cache_image();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let tmp = dest.with_extension("part");

    let args = curl_args(&url, &tmp);
    let output = Command::new(&curl)
        .args(&args)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;

    let mut lines = capture_lines(&output.stdout, &output.stderr);
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        lines.push(format!(
            "download failed ({}) from {url} — no published image yet? Set {FIRMWARE_URL_ENV} \
             to a working URL, or build the firmware from source instead.",
            crate::flash::exit_desc(output.status.code())
        ));
        return Err(lines.join("\n"));
    }

    let size = std::fs::metadata(&tmp).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&tmp);
        return Err(format!("downloaded image from {url} is empty — refusing to flash it."));
    }
    std::fs::rename(&tmp, &dest)
        .map_err(|e| format!("cannot move the download into place: {e}"))?;

    lines.push(format!("downloaded {size} bytes to {}", dest.display()));
    Ok((dest, lines))
}

/// `curl` argv for fetching the firmware: follow redirects (the "latest
/// release" URL is one), fail on HTTP errors, retry transient ones, stay quiet
/// but keep real error text.
pub fn curl_args(url: &str, dest: &Path) -> Vec<String> {
    vec![
        "--fail".into(),
        "--location".into(),
        "--retry".into(),
        "3".into(),
        "--silent".into(),
        "--show-error".into(),
        "--output".into(),
        dest.display().to_string(),
        url.to_string(),
    ]
}

/// Merge a command's stdout and stderr into display lines.
fn capture_lines(stdout: &[u8], stderr: &[u8]) -> Vec<String> {
    String::from_utf8_lossy(stdout)
        .lines()
        .chain(String::from_utf8_lossy(stderr).lines())
        .map(|l| l.to_string())
        .collect()
}

/// Which firmware sources are usable right now, for the wizard's picker.
#[derive(Debug, Clone)]
pub struct Sources {
    pub toolchain: Toolchain,
    /// `firmware/` crate directory, when running from a source checkout.
    pub firmware_dir: Option<PathBuf>,
    /// An already-built or already-downloaded image, if one exists.
    pub existing: Option<PathBuf>,
    pub url: String,
    pub have_curl: bool,
}

impl Sources {
    pub fn detect() -> Sources {
        Sources {
            toolchain: toolchain(),
            firmware_dir: firmware_dir(),
            existing: crate::flash::resolve_image(None).ok(),
            url: firmware_url(),
            have_curl: crate::flash::which(&["curl"]).is_some(),
        }
    }

    /// Whether a from-source build can even be attempted.
    pub fn can_build(&self) -> bool {
        self.toolchain.is_ready() && self.firmware_dir.is_some()
    }

    /// Why building is unavailable, when it is.
    pub fn build_blocker(&self) -> Option<String> {
        if !self.toolchain.is_ready() {
            return Some(self.toolchain.describe());
        }
        if self.firmware_dir.is_none() {
            return Some("no firmware/ directory here — run from an OpenMicro checkout".into());
        }
        None
    }

    /// Whether a download can be attempted.
    pub fn can_download(&self) -> bool {
        self.have_curl
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_script_sources_the_export_and_builds_release() {
        let s = build_script(Path::new("/w/firmware"), Path::new("/home/u/export-esp.sh"));
        assert!(s.contains(". '/home/u/export-esp.sh'"), "{s}");
        assert!(s.contains("cd '/w/firmware'"), "{s}");
        assert!(s.contains("cargo build --release"), "{s}");
        assert!(s.starts_with("set -e"), "a failed source must abort the build: {s}");
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("a'b"), r"'a'\''b'");
        // The quoted form round-trips through a real shell.
        let out = Command::new("sh")
            .arg("-c")
            .arg(format!("printf %s {}", shell_quote("we'ird dir")))
            .output()
            .unwrap();
        assert_eq!(String::from_utf8_lossy(&out.stdout), "we'ird dir");
    }

    #[test]
    fn curl_args_fail_on_http_errors_and_follow_redirects() {
        let args = curl_args("https://example/fw.bin", Path::new("/tmp/fw.part"));
        assert!(args.contains(&"--fail".to_string()), "must not save 404 bodies");
        assert!(args.contains(&"--location".to_string()), "latest-release URL redirects");
        assert_eq!(args.last().unwrap(), "https://example/fw.bin");
        let out_at = args.iter().position(|a| a == "--output").unwrap();
        assert_eq!(args[out_at + 1], "/tmp/fw.part");
    }

    #[test]
    fn firmware_url_honors_the_env_override() {
        // Serialized implicitly: this is the only test touching the var.
        std::env::set_var(FIRMWARE_URL_ENV, "https://elsewhere/fw.bin");
        assert_eq!(firmware_url(), "https://elsewhere/fw.bin");
        std::env::set_var(FIRMWARE_URL_ENV, "   ");
        assert_eq!(firmware_url(), DEFAULT_FIRMWARE_URL, "blank falls back to the default");
        std::env::remove_var(FIRMWARE_URL_ENV);
        assert_eq!(firmware_url(), DEFAULT_FIRMWARE_URL);
    }

    #[test]
    fn toolchain_describe_is_actionable() {
        assert!(Toolchain::Missing.describe().contains("espup install"));
        assert!(Toolchain::NeedsInstall.describe().contains("espup install"));
        assert!(Toolchain::Ready(PathBuf::from("/x")).describe().contains("/x"));
        assert!(!Toolchain::Missing.is_ready());
        assert!(Toolchain::Ready(PathBuf::from("/x")).is_ready());
    }

    #[test]
    fn cache_image_is_under_the_users_cache_dir() {
        assert!(cache_image().ends_with(CACHE_REL));
    }

    #[test]
    fn sources_detect_reports_blockers_without_panicking() {
        let s = Sources::detect();
        // On a machine without the Xtensa toolchain (this one, and most), the
        // build path must be unavailable *with a reason*, never silently ok.
        assert_eq!(s.can_build(), s.build_blocker().is_none());
    }
}

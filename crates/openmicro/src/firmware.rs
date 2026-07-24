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

/// GitHub API listing every published release, newest first. The firmware
/// images are built and attached by `.github/workflows/firmware.yml`.
pub const RELEASES_API: &str = "https://api.github.com/repos/SilkePilon/OpenMicro/releases";

/// Environment variable that overrides [`RELEASES_API`], for a fork or a local
/// mirror (a `file://` path works too, which is how the parser is exercised).
pub const RELEASES_URL_ENV: &str = "OPENMICRO_RELEASES_URL";

/// Environment variable that bypasses the release list entirely and downloads
/// one specific URL. Useful for a self-hosted or locally built image.
pub const FIRMWARE_URL_ENV: &str = "OPENMICRO_FIRMWARE_URL";

/// Work Louder publish the **stock** Creator Micro 2 firmware here, as
/// unencrypted merged flash images. This is what makes going back to the
/// vendor firmware possible without having taken a backup first.
pub const STOCK_RELEASES_API: &str =
    "https://api.github.com/repos/worklouder/cm-v2-fw-releases/releases";

/// Override for [`STOCK_RELEASES_API`] (a fork, a mirror, or a `file://` path).
pub const STOCK_RELEASES_URL_ENV: &str = "OPENMICRO_STOCK_RELEASES_URL";

/// Sibling file recording which release the cached image came from, so the CLI
/// and the wizard can say *which version* is sitting in the cache.
pub const CACHE_VERSION_REL: &str = ".cache/openmicro/firmware/openmicro-fw.version";

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

/// Path of the file recording the cached image's release tag.
pub fn cache_version_file() -> PathBuf {
    crate::agents::home().join(CACHE_VERSION_REL)
}

/// The release tag of the cached image, if one was downloaded.
pub fn cached_version() -> Option<String> {
    std::fs::read_to_string(cache_version_file())
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A direct download URL forced by `$OPENMICRO_FIRMWARE_URL`, if set. When
/// present it bypasses the release list entirely.
pub fn forced_url() -> Option<String> {
    std::env::var(FIRMWARE_URL_ENV).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

/// The API endpoint the release list is fetched from.
pub fn releases_url() -> String {
    std::env::var(RELEASES_URL_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| RELEASES_API.to_string())
}

/// One published firmware release.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// Git tag, e.g. `v1.2.0`. The stable identifier users pick by.
    pub tag: String,
    /// Human title; falls back to the tag when GitHub has none.
    pub name: String,
    pub prerelease: bool,
    /// ISO-8601 publish timestamp, or empty when absent.
    pub published: String,
    /// Download URL of the firmware asset, when the release has one. A release
    /// whose build failed (or predates the workflow) has none, and cannot be
    /// installed — the picker shows it greyed out rather than hiding it.
    pub asset_url: Option<String>,
    /// Size of that asset in bytes.
    pub asset_size: u64,
}

impl Release {
    /// One-line label for the picker.
    pub fn label(&self) -> String {
        let date = self.published.split('T').next().unwrap_or("").to_string();
        let kind = if self.prerelease { "  (pre-release)" } else { "" };
        let title = if self.name.is_empty() || self.name == self.tag {
            String::new()
        } else {
            format!("  {}", self.name)
        };
        format!("{}{title}{kind}   {date}", self.tag)
    }

    /// Why this release cannot be installed, when it cannot.
    pub fn blocker(&self) -> Option<&'static str> {
        self.asset_url.is_none().then_some("no firmware asset attached to this release")
    }
}

/// Parse the GitHub releases JSON into [`Release`]s, newest first.
///
/// Pure, so the shape of the API response is pinned by tests rather than
/// discovered at runtime. Drafts are dropped (they are not downloadable), and
/// a release with no `.bin` asset is kept but marked uninstallable, which is
/// more honest than silently omitting a version the user can see on GitHub.
pub fn parse_releases(json: &str) -> Result<Vec<Release>, String> {
    parse_releases_matching(json, is_firmware_asset)
}

/// Parse the vendor's stock-firmware release list.
pub fn parse_stock_releases(json: &str) -> Result<Vec<Release>, String> {
    parse_releases_matching(json, is_stock_asset)
}

/// Shared release-list parser, differing only in what counts as the image.
fn parse_releases_matching(
    json: &str,
    is_image: fn(&serde_json::Value) -> bool,
) -> Result<Vec<Release>, String> {
    let value: serde_json::Value = serde_json::from_str(json)
        .map_err(|e| format!("release list is not valid JSON ({e})"))?;
    let items = value.as_array().ok_or_else(|| {
        // A rate-limited or errored API responds with an object, not an array.
        let msg = value.get("message").and_then(|m| m.as_str()).unwrap_or("unexpected response");
        format!("could not list releases: {msg}")
    })?;

    let mut out = Vec::new();
    for item in items {
        if item.get("draft").and_then(|d| d.as_bool()).unwrap_or(false) {
            continue;
        }
        let Some(tag) = item.get("tag_name").and_then(|t| t.as_str()) else { continue };
        let name = item.get("name").and_then(|n| n.as_str()).unwrap_or(tag);
        let asset = item
            .get("assets")
            .and_then(|a| a.as_array())
            .and_then(|assets| assets.iter().find(|a| is_image(a)));

        out.push(Release {
            tag: tag.to_string(),
            name: name.to_string(),
            prerelease: item.get("prerelease").and_then(|p| p.as_bool()).unwrap_or(false),
            published: item
                .get("published_at")
                .and_then(|p| p.as_str())
                .unwrap_or_default()
                .to_string(),
            asset_url: asset
                .and_then(|a| a.get("browser_download_url"))
                .and_then(|u| u.as_str())
                .map(str::to_string),
            asset_size: asset.and_then(|a| a.get("size")).and_then(|s| s.as_u64()).unwrap_or(0),
        });
    }
    Ok(out)
}

/// True for an asset that looks like one of *our* flashable images.
fn is_firmware_asset(asset: &serde_json::Value) -> bool {
    asset
        .get("name")
        .and_then(|n| n.as_str())
        .is_some_and(|n| n.ends_with(".bin") && n.contains("openmicro"))
}

/// True for a vendor **merged** flash image, e.g.
/// `firmware_v0.6.0-rc.6_merged.bin`.
///
/// The "merged" part matters: the vendor also ships per-partition images, and
/// only the merged one can be written at offset `0x0` the way [`crate::flash`]
/// does it.
fn is_stock_asset(asset: &serde_json::Value) -> bool {
    asset
        .get("name")
        .and_then(|n| n.as_str())
        .is_some_and(|n| n.ends_with(".bin") && n.contains("merged"))
}

/// The API endpoint the stock-firmware list is fetched from.
pub fn stock_releases_url() -> String {
    std::env::var(STOCK_RELEASES_URL_ENV)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| STOCK_RELEASES_API.to_string())
}

/// Fetch the vendor's published stock-firmware releases, newest first.
pub fn fetch_stock_releases() -> Result<Vec<Release>, String> {
    parse_stock_releases(&fetch_release_json(&stock_releases_url())?)
}

/// Where a downloaded stock image is cached. Kept per-version, and separate
/// from our own image, so restoring never races with flashing OpenMicro.
pub fn stock_cache_image(tag: &str) -> PathBuf {
    let safe: String = tag
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' { c } else { '_' })
        .collect();
    crate::agents::home().join(".cache/openmicro/firmware").join(format!("stock-{safe}.bin"))
}

/// Download one vendor stock-firmware release.
pub fn download_stock_release(release: &Release) -> Result<(PathBuf, Vec<String>), String> {
    let url = release.asset_url.as_ref().ok_or_else(|| {
        format!("vendor release {} has no merged image attached.", release.tag)
    })?;
    let dest = stock_cache_image(&release.tag);
    fetch_to(url, &dest, &format!("stock firmware {}", release.tag))
}

/// Fetch and parse our own published release list.
pub fn fetch_releases() -> Result<Vec<Release>, String> {
    parse_releases(&fetch_release_json(&releases_url())?)
}

/// GET a GitHub release listing and return its body.
fn fetch_release_json(url: &str) -> Result<String, String> {
    let curl = crate::flash::which(&["curl"])
        .ok_or_else(|| "curl not found — install curl to list firmware releases.".to_string())?;
    let output = std::process::Command::new(&curl)
        .args(release_list_args(url))
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "could not fetch the release list from {url} ({}): {}",
            crate::flash::exit_desc(output.status.code()),
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// `curl` argv for the release list: GitHub's API requires a User-Agent and
/// rejects requests without one.
pub fn release_list_args(url: &str) -> Vec<String> {
    vec![
        "--fail".into(),
        "--location".into(),
        "--silent".into(),
        "--show-error".into(),
        "--user-agent".into(),
        concat!("openmicro/", env!("CARGO_PKG_VERSION")).into(),
        "--header".into(),
        "Accept: application/vnd.github+json".into(),
        url.to_string(),
    ]
}

/// Choose which release to install.
///
/// With an explicit `version`, only an exact tag match will do — silently
/// falling back to a different version is the last thing anyone wants from a
/// firmware installer. Without one, the newest release that actually has an
/// asset wins, preferring stable over pre-release.
pub fn pick_release<'a>(
    releases: &'a [Release],
    version: Option<&str>,
) -> Result<&'a Release, String> {
    if let Some(tag) = version {
        return releases
            .iter()
            .find(|r| r.tag == tag)
            .ok_or_else(|| match releases.iter().map(|r| r.tag.as_str()).collect::<Vec<_>>() {
                tags if tags.is_empty() => format!("no release {tag}: no releases published yet"),
                tags => format!("no release {tag}. Available: {}", tags.join(", ")),
            });
    }
    releases
        .iter()
        .find(|r| !r.prerelease && r.asset_url.is_some())
        .or_else(|| releases.iter().find(|r| r.asset_url.is_some()))
        .ok_or_else(|| {
            "no published release has a firmware asset yet — build from source instead."
                .to_string()
        })
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

/// Download the firmware for `version` (or the newest suitable release).
///
/// With `$OPENMICRO_FIRMWARE_URL` set, that URL is fetched directly and the
/// release list is not consulted at all.
pub fn download(version: Option<&str>) -> Result<(PathBuf, Vec<String>), String> {
    if let Some(url) = forced_url() {
        return download_from(&url, &format!("({FIRMWARE_URL_ENV})"));
    }
    let releases = fetch_releases()?;
    let release = pick_release(&releases, version)?;
    download_release(release)
}

/// Download one specific release's firmware asset.
pub fn download_release(release: &Release) -> Result<(PathBuf, Vec<String>), String> {
    let url = release.asset_url.as_ref().ok_or_else(|| {
        format!(
            "release {} has no firmware asset attached — pick another version, or build \
             from source.",
            release.tag
        )
    })?;
    let result = download_from(url, &release.tag);
    if result.is_ok() {
        // Best-effort note of which version is now cached; a failure here only
        // costs us the ability to display it later.
        let _ = std::fs::write(cache_version_file(), format!("{}\n", release.tag));
    }
    result
}

/// Fetch `url` into the firmware cache.
///
/// Uses `curl` (present on every machine that can install this project) with
/// `--fail`, so an HTML 404 page is never mistaken for firmware. The file is
/// written to a temp path and renamed only after a successful transfer, so a
/// half-downloaded image can never be flashed.
fn download_from(url: &str, label: &str) -> Result<(PathBuf, Vec<String>), String> {
    fetch_to(url, &cache_image(), label)
}

/// Fetch `url` to `dest`, atomically.
fn fetch_to(url: &str, dest: &Path, label: &str) -> Result<(PathBuf, Vec<String>), String> {
    let curl = crate::flash::which(&["curl"])
        .ok_or_else(|| "curl not found — install curl, or build the firmware instead.".to_string())?;
    let dest = dest.to_path_buf();
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    }
    let tmp = dest.with_extension("part");

    let args = curl_args(url, &tmp);
    let output = Command::new(&curl)
        .args(&args)
        .output()
        .map_err(|e| format!("cannot run curl: {e}"))?;

    let mut lines = capture_lines(&output.stdout, &output.stderr);
    if !output.status.success() {
        let _ = std::fs::remove_file(&tmp);
        lines.push(format!(
            "download failed ({}) from {url} — build the firmware from source instead, or \
             set {FIRMWARE_URL_ENV} to a working URL.",
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

    lines.push(format!("downloaded {label}: {size} bytes to {}", dest.display()));
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
    /// Where firmware would be fetched from: the release list, or the URL
    /// `$OPENMICRO_FIRMWARE_URL` forces.
    pub url: String,
    /// True when `$OPENMICRO_FIRMWARE_URL` forces one URL and the release list
    /// is bypassed — the version picker has nothing to offer in that case.
    pub forced: bool,
    pub have_curl: bool,
}

impl Sources {
    pub fn detect() -> Sources {
        let forced = forced_url();
        Sources {
            toolchain: toolchain(),
            firmware_dir: firmware_dir(),
            url: forced.clone().unwrap_or_else(releases_url),
            forced: forced.is_some(),
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

    /// Environment variables are process-wide, but `cargo test` runs tests on
    /// parallel threads: every test that sets one must hold this, or another
    /// test will observe it. (Without it, `fetch_releases_parses_a_local_file_url`
    /// and this test raced over `OPENMICRO_RELEASES_URL`.) Poison-tolerant, so
    /// one failing test does not cascade into the rest.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner())
    }

    #[test]
    fn firmware_url_honors_the_env_override() {
        let _guard = lock_env();
        std::env::set_var(FIRMWARE_URL_ENV, "https://elsewhere/fw.bin");
        assert_eq!(forced_url().as_deref(), Some("https://elsewhere/fw.bin"));
        std::env::set_var(FIRMWARE_URL_ENV, "   ");
        assert_eq!(forced_url(), None, "blank is not an override");
        std::env::remove_var(FIRMWARE_URL_ENV);
        assert_eq!(forced_url(), None);
        assert_eq!(releases_url(), RELEASES_API);
    }

    /// A trimmed-down copy of what the GitHub releases API returns.
    const RELEASES_JSON: &str = r#"[
      {"tag_name":"v1.2.0","name":"OpenMicro 1.2.0","draft":false,"prerelease":false,
       "published_at":"2026-07-20T10:00:00Z",
       "assets":[{"name":"openmicro-fw.bin","size":812345,
                  "browser_download_url":"https://example/v1.2.0/openmicro-fw.bin"},
                 {"name":"sha256sums.txt","size":90,
                  "browser_download_url":"https://example/v1.2.0/sha256sums.txt"}]},
      {"tag_name":"v1.3.0-rc1","name":null,"draft":false,"prerelease":true,
       "published_at":"2026-07-22T10:00:00Z",
       "assets":[{"name":"openmicro-fw.bin","size":812999,
                  "browser_download_url":"https://example/v1.3.0-rc1/openmicro-fw.bin"}]},
      {"tag_name":"v1.1.0","name":"OpenMicro 1.1.0","draft":false,"prerelease":false,
       "published_at":"2026-07-10T10:00:00Z","assets":[]},
      {"tag_name":"v9.9.9","name":"unfinished","draft":true,"prerelease":false,
       "published_at":null,"assets":[]}
    ]"#;

    #[test]
    fn parse_releases_reads_tags_assets_and_flags() {
        let rels = parse_releases(RELEASES_JSON).unwrap();
        assert_eq!(rels.len(), 3, "the draft must be dropped");
        assert_eq!(rels[0].tag, "v1.2.0");
        assert_eq!(
            rels[0].asset_url.as_deref(),
            Some("https://example/v1.2.0/openmicro-fw.bin"),
            "must pick the firmware .bin, not the checksums file"
        );
        assert_eq!(rels[0].asset_size, 812_345);
        assert!(!rels[0].prerelease);

        assert!(rels[1].prerelease);
        assert_eq!(rels[1].name, "v1.3.0-rc1", "a null name falls back to the tag");

        // A release with no firmware asset is kept, but marked uninstallable —
        // hiding a version the user can see on GitHub would be confusing.
        assert_eq!(rels[2].tag, "v1.1.0");
        assert!(rels[2].asset_url.is_none());
        assert!(rels[2].blocker().is_some());
    }

    #[test]
    fn parse_releases_reports_an_api_error_object() {
        let err = parse_releases(r#"{"message":"API rate limit exceeded"}"#).unwrap_err();
        assert!(err.contains("rate limit"), "{err}");
    }

    #[test]
    fn parse_releases_rejects_garbage() {
        assert!(parse_releases("not json").unwrap_err().contains("not valid JSON"));
        assert!(parse_releases("").is_err());
    }

    #[test]
    fn parse_releases_of_an_empty_list_is_empty() {
        assert!(parse_releases("[]").unwrap().is_empty());
    }

    #[test]
    fn pick_release_prefers_the_newest_stable_with_an_asset() {
        let rels = parse_releases(RELEASES_JSON).unwrap();
        // v1.3.0-rc1 is newer but a pre-release; v1.1.0 is stable but has no
        // asset. v1.2.0 is the only correct answer.
        assert_eq!(pick_release(&rels, None).unwrap().tag, "v1.2.0");
    }

    #[test]
    fn pick_release_falls_back_to_a_prerelease_when_nothing_else_has_an_asset() {
        let rels: Vec<Release> = parse_releases(RELEASES_JSON)
            .unwrap()
            .into_iter()
            .filter(|r| r.tag != "v1.2.0")
            .collect();
        assert_eq!(pick_release(&rels, None).unwrap().tag, "v1.3.0-rc1");
    }

    #[test]
    fn pick_release_matches_an_explicit_tag_exactly() {
        let rels = parse_releases(RELEASES_JSON).unwrap();
        assert_eq!(pick_release(&rels, Some("v1.3.0-rc1")).unwrap().tag, "v1.3.0-rc1");

        // Never silently substitute a different version.
        let err = pick_release(&rels, Some("v0.0.1")).unwrap_err();
        assert!(err.contains("no release v0.0.1"), "{err}");
        assert!(err.contains("v1.2.0"), "must list what is available: {err}");
    }

    #[test]
    fn pick_release_with_nothing_installable_says_so() {
        let err = pick_release(&[], None).unwrap_err();
        assert!(err.contains("build from source"), "{err}");
    }

    #[test]
    fn release_label_is_readable() {
        let rels = parse_releases(RELEASES_JSON).unwrap();
        let stable = rels[0].label();
        assert!(stable.contains("v1.2.0") && stable.contains("2026-07-20"), "{stable}");
        assert!(!stable.contains("pre-release"));
        assert!(rels[1].label().contains("(pre-release)"), "{}", rels[1].label());
    }

    #[test]
    fn release_list_args_identify_the_client() {
        // GitHub rejects API requests without a User-Agent.
        let args = release_list_args("https://api/x");
        assert!(args.contains(&"--user-agent".to_string()), "{args:?}");
        assert!(args.contains(&"--fail".to_string()));
        assert_eq!(args.last().unwrap(), "https://api/x");
    }

    #[test]
    fn fetch_releases_parses_a_local_file_url() {
        // End-to-end through curl, without touching the network: curl reads
        // file:// URLs, so this exercises the real fetch + parse path.
        let _guard = lock_env();
        let path = std::env::temp_dir()
            .join(format!("openmicro-releases-{}.json", std::process::id()));
        std::fs::write(&path, RELEASES_JSON).unwrap();
        std::env::set_var(RELEASES_URL_ENV, format!("file://{}", path.display()));

        let result = fetch_releases();

        std::env::remove_var(RELEASES_URL_ENV);
        let _ = std::fs::remove_file(&path);

        let rels = result.unwrap();
        assert_eq!(rels.len(), 3);
        assert_eq!(rels[0].tag, "v1.2.0");
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

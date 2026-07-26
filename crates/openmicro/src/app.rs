//! The application: one command, one menu.
//!
//! `openmicro` takes no subcommands. Everything — setting the device up,
//! flashing firmware, wiring coding agents, running the daemon, removing the
//! whole thing again — happens inside this menu loop, in the style described by
//! `docs/tui-style.md`.
//!
//! The shape is a linear prompt transcript rather than a full-screen app: each
//! action appends its own record and returns to the menu, so the terminal ends
//! up holding a readable history of what was done.

use std::path::PathBuf;

use crate::agents::{self, AgentKind, HookStatus};
use crate::daemon;
use crate::firmware::{self, Release};
use crate::flash;
use crate::client;
use crate::probe;
use crate::prompt::{Cancelled, Item, MultiOpts, SelectOption, Ui};
use crate::uninstall::{self, Target};
use crate::wldevice::{self, UsbMode};

/// The wordmark, in figlet's ANSI Shadow typeface to match `npx skills`.
pub const WORDMARK: [&str; 6] = [
    " ██████╗ ██████╗ ███████╗███╗   ██╗███╗   ███╗██╗ ██████╗██████╗  ██████╗ ",
    "██╔═══██╗██╔══██╗██╔════╝████╗  ██║████╗ ████║██║██╔════╝██╔══██╗██╔═══██╗",
    "██║   ██║██████╔╝█████╗  ██╔██╗ ██║██╔████╔██║██║██║     ██████╔╝██║   ██║",
    "██║   ██║██╔═══╝ ██╔══╝  ██║╚██╗██║██║╚██╔╝██║██║██║     ██╔══██╗██║   ██║",
    "╚██████╔╝██║     ███████╗██║ ╚████║██║ ╚═╝ ██║██║╚██████╗██║  ██║╚██████╔╝",
    " ╚═════╝ ╚═╝     ╚══════╝╚═╝  ╚═══╝╚═╝     ╚═╝╚═╝ ╚═════╝╚═╝  ╚═╝ ╚═════╝ ",
];

/// Terminal width below which the wordmark is skipped rather than wrapped into
/// nonsense.
const WORDMARK_MIN_COLUMNS: usize = 78;

/// What the user picked from the top-level menu.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Action {
    Setup,
    Live,
    Settings,
    Firmware,
    Agents,
    Daemon,
    Device,
    Uninstall,
    Quit,
}

/// A cheap read of the things the menu describes.
///
/// Deliberately USB-only: a Bluetooth scan takes seconds, and redrawing the
/// menu after every action must feel instant. The guided setup does the slow,
/// thorough probe behind a spinner instead.
#[derive(Clone)]
struct Snapshot {
    usb: UsbMode,
    daemon: daemon::Status,
    agents_missing: usize,
    agents_installed: usize,
    image: Option<PathBuf>,
    cached_version: Option<String>,
}

impl Snapshot {
    fn read() -> Snapshot {
        let rows = agents::detect(&agents::home());
        Snapshot {
            usb: wldevice::usb_mode(),
            daemon: daemon::status(),
            agents_missing: rows
                .iter()
                .filter(|r| r.present && r.status == HookStatus::Missing)
                .count(),
            agents_installed: rows
                .iter()
                .filter(|r| r.status == HookStatus::Installed)
                .count(),
            image: flash::resolve_image(None).ok(),
            cached_version: firmware::cached_version(),
        }
    }

    fn device_hint(&self) -> String {
        match self.usb {
            UsbMode::App(pid) => format!("{} connected", wldevice::product_name(pid)),
            UsbMode::Bootloader => "in bootloader mode".to_string(),
            UsbMode::Absent => "not connected".to_string(),
        }
    }

    fn firmware_hint(&self) -> String {
        match (&self.image, &self.cached_version) {
            (Some(_), Some(v)) => format!("{v} downloaded"),
            (Some(p), None) => format!("image at {}", short_path(p)),
            (None, _) => "no image yet".to_string(),
        }
    }

    fn agents_hint(&self) -> String {
        match (self.agents_installed, self.agents_missing) {
            (0, 0) => "no supported agents found".to_string(),
            (n, 0) => format!("{n} wired up"),
            (0, m) => format!("{m} detected, none wired up"),
            (n, m) => format!("{n} wired up, {m} still missing"),
        }
    }
}

/// Abbreviate a path with `~` so menu hints stay on one line.
fn short_path(path: &std::path::Path) -> String {
    let home = agents::home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Entry point. Returns once the user quits.
pub fn run() -> anyhow::Result<()> {
    let mut ui = Ui::new();
    if ui.columns() >= WORDMARK_MIN_COLUMNS {
        ui.banner(&WORDMARK);
    }
    // Black on cyan, like `skills`' own badge. Colour 0 is black in the 256
    // palette; the toolkit has no dedicated `black` helper.
    let badge = ui.style().bg_cyan(&ui.style().fg256(0, " openmicro "));
    ui.intro(&badge);

    // Offer to start the daemon before the menu, since almost everything else
    // is more useful with it running.
    if let Err(Cancelled) = offer_to_start_daemon(&mut ui) {
        ui.cancel("Cancelled");
        return Ok(());
    }

    loop {
        let snapshot = Snapshot::read();
        let action = match main_menu(&mut ui, &snapshot) {
            Ok(a) => a,
            Err(Cancelled) => break,
        };
        let outcome = match action {
            Action::Setup => guided_setup(&mut ui),
            Action::Live => live_status(&mut ui),
            Action::Settings => settings_menu(&mut ui),
            Action::Firmware => firmware_menu(&mut ui, &snapshot),
            Action::Agents => agents_flow(&mut ui),
            Action::Daemon => daemon_menu(&mut ui, &snapshot),
            Action::Device => device_menu(&mut ui, &snapshot),
            Action::Uninstall => uninstall_flow(&mut ui),
            Action::Quit => break,
        };
        if outcome.is_err() {
            // Cancelling one action returns to the menu rather than quitting;
            // quitting is its own menu entry, so a stray Esc never loses work.
            ui.warn("Cancelled — back to the menu.");
        }
    }

    ui.outro("Bye.");
    Ok(())
}

/// The top-level menu, with live state in the hints.
fn main_menu(ui: &mut Ui, snapshot: &Snapshot) -> Result<Action, Cancelled> {
    let options = vec![
        SelectOption::new(Action::Setup, "Set up my macropad")
            .with_hint("firmware, then coding agents — start here"),
        SelectOption::new(Action::Live, "Watch agent activity").with_hint(
            if snapshot.daemon.running { "live view" } else { "needs the service running" },
        ),
        SelectOption::new(Action::Settings, "Lights and sleep")
            .with_hint("brightness, per-state colours, idle timeout"),
        SelectOption::new(Action::Firmware, "Firmware").with_hint(snapshot.firmware_hint()),
        SelectOption::new(Action::Agents, "Coding agents").with_hint(snapshot.agents_hint()),
        SelectOption::new(Action::Daemon, "Background service")
            .with_hint(snapshot.daemon.describe()),
        SelectOption::new(Action::Device, "Device").with_hint(snapshot.device_hint()),
        SelectOption::new(Action::Uninstall, "Uninstall OpenMicro")
            .with_hint("remove everything from this computer"),
        SelectOption::new(Action::Quit, "Quit"),
    ];
    ui.select("What do you want to do?", &options)
}

/// Ask once, at startup, whether to start a daemon that is installed but down.
///
/// Silent when there is nothing to offer: no service installed means the
/// question would only be noise, and the menu says so anyway.
fn offer_to_start_daemon(ui: &mut Ui) -> Result<(), Cancelled> {
    let status = daemon::status();
    if status.running || !status.unit_installed {
        return Ok(());
    }
    if !ui.confirm("The background service isn't running. Start it now?", true)? {
        return Ok(());
    }
    run_job(ui, "Starting the background service", "Service running", daemon::start);
    Ok(())
}

/// Run a fallible job behind a spinner and report its output.
///
/// Everything slow in this app has the same shape — spin, produce log lines or
/// an error, print them — so it lives in one place and every action reports
/// success and failure the same way.
fn run_job<F>(ui: &mut Ui, busy: &str, done: &str, job: F) -> bool
where
    F: FnOnce() -> Result<Vec<String>, String>,
{
    let mut spinner = ui.spinner();
    spinner.start(busy);
    let result = job();
    match result {
        Ok(lines) => {
            spinner.stop(done);
            for line in lines.iter().filter(|l| !l.trim().is_empty()) {
                ui.info(line);
            }
            true
        }
        Err(e) => {
            spinner.error(busy);
            // Tool output is mostly progress chatter; only the last line is the
            // failure. Rendering all of it as errors buries the one that matters
            // in a wall of red markers.
            let lines: Vec<&str> = e.lines().filter(|l| !l.trim().is_empty()).collect();
            if let Some((last, rest)) = lines.split_last() {
                for line in rest {
                    ui.info(line);
                }
                ui.error(last);
            }
            false
        }
    }
}

// ---------------------------------------------------------------------------
// Guided setup
// ---------------------------------------------------------------------------

/// The path the user is meant to take: device to working, asking as little as
/// possible along the way.
fn guided_setup(ui: &mut Ui) -> Result<(), Cancelled> {
    ui.note(
        "Before you start",
        "OpenMicro replaces the firmware on your macropad with its own build.\n\
         \n\
         This is not something the vendor supports, and it may void your warranty.\n\
         \n\
         Going back is straightforward: Work Louder publish their firmware openly,\n\
         and Firmware > Restore the stock firmware downloads and writes it. Taking\n\
         a backup first is still offered, since that keeps your exact current image\n\
         rather than a stock one.",
    );
    if !ui.confirm("Understood — continue?", false)? {
        return Ok(());
    }

    // 1. Find the device.
    let mut spinner = ui.spinner();
    spinner.start("Looking for your macropad");
    let seen = probe::probe();
    let usb = wldevice::usb_mode();
    match (usb, seen.firmware()) {
        (UsbMode::Absent, probe::FirmwareKind::OpenMicro) => {
            spinner.stop("Found it over Bluetooth, already running OpenMicro firmware");
            ui.info("Nothing to flash. Moving on to your coding agents.");
            return finish_setup(ui);
        }
        (UsbMode::Absent, _) => {
            spinner.error("No macropad found");
            ui.error(
                "Connect it over USB with a data-capable cable — charge-only cables \
                 enumerate nothing.",
            );
            return Ok(());
        }
        (UsbMode::Bootloader, _) => spinner.stop("Found it, already in bootloader mode"),
        (UsbMode::App(pid), _) => {
            spinner.stop(&format!("Found {}", wldevice::product_name(pid)));
        }
    }

    // 2. Get it into bootloader mode. No buttons: the firmware is asked to
    //    reboot itself.
    if wldevice::usb_mode() != UsbMode::Bootloader {
        if !ui.confirm("Reboot it into bootloader mode so it can be flashed?", true)? {
            return Ok(());
        }
        if !run_job(
            ui,
            "Rebooting into bootloader mode",
            "In bootloader mode",
            wldevice::enter_bootloader,
        ) {
            return Ok(());
        }
    }

    // 3. Offer the backup while we still can — after flashing it is too late.
    let backup = flash::backup_path();
    if backup.is_file() {
        ui.step(&format!("Stock firmware already backed up to {}", short_path(&backup)));
    } else if ui.confirm("Back up your current firmware first? (optional)", false)?
        && port_is_clear(ui)?
    {
        run_job(ui, "Reading the stock firmware", "Stock firmware saved", || {
            flash::backup(None, None).map(|(_, lines)| lines)
        });
    }

    // 4. Get an image, then write it.
    if obtain_firmware(ui)?.is_none() {
        return Ok(());
    }
    if !ui.confirm("Write this firmware to the device now?", true)? || !port_is_clear(ui)? {
        return Ok(());
    }
    if !run_job(ui, "Flashing", "Firmware written", || flash::flash_capture(None, None)) {
        return Ok(());
    }
    // The write deliberately leaves the device in the bootloader: entering it
    // set the force-download bit, which survives a reset, so a device that
    // simply rebooted here would come straight back to download mode and never
    // run the firmware that was just written. Clearing the bit and resetting is
    // what actually starts it.
    run_job(ui, "Starting the new firmware", "Device restarted", || {
        wldevice::exit_bootloader(None)
    });

    finish_setup(ui)
}

/// The tail of the guided setup: agents, then the daemon.
fn finish_setup(ui: &mut Ui) -> Result<(), Cancelled> {
    agents_flow(ui)?;
    let status = daemon::status();
    if !status.running && status.unit_installed && ui.confirm("Start the background service?", true)?
    {
        run_job(ui, "Starting the background service", "Service running", daemon::start);
    }
    ui.success("Setup complete.");
    Ok(())
}

// ---------------------------------------------------------------------------
// Firmware
// ---------------------------------------------------------------------------

/// Make sure a firmware image exists, building or downloading one if not.
/// Returns `None` when the user backed out or nothing could be obtained.
fn obtain_firmware(ui: &mut Ui) -> Result<Option<PathBuf>, Cancelled> {
    if let Ok(existing) = flash::resolve_image(None) {
        let hint = match firmware::cached_version() {
            Some(v) => format!("version {v}"),
            None => short_path(&existing),
        };
        let options = vec![
            SelectOption::new(true, "Use the firmware I already have").with_hint(hint),
            SelectOption::new(false, "Get a different version"),
        ];
        if ui.select("Which firmware?", &options)? {
            return Ok(Some(existing));
        }
    }
    firmware_source(ui)
}

/// Download a published release or build from source.
fn firmware_source(ui: &mut Ui) -> Result<Option<PathBuf>, Cancelled> {
    let sources = firmware::Sources::detect();
    let mut options = Vec::new();
    if sources.can_download() {
        // A forced URL bypasses the release list entirely, so there is no
        // version to pick — say where the image will come from instead.
        if sources.forced {
            options.push(
                SelectOption::new("forced", "Download the configured image")
                    .with_hint(sources.url.clone()),
            );
        } else {
            options.push(
                SelectOption::new("download", "Download a published version")
                    .with_hint("pick from the releases"),
            );
        }
    }
    options.push(if sources.can_build() {
        SelectOption::new("build", "Build from source")
            .with_hint("compiles firmware/ with the Xtensa toolchain")
    } else {
        SelectOption::new("blocked", "Build from source")
            .with_hint(sources.build_blocker().unwrap_or_default())
    });
    options.push(SelectOption::new("back", "Back"));

    match ui.select("Where should the firmware come from?", &options)? {
        "download" => download_release(ui),
        "forced" => {
            let mut path = None;
            run_job(ui, "Downloading firmware", "Firmware downloaded", || {
                firmware::download(None).map(|(p, lines)| {
                    path = Some(p);
                    lines
                })
            });
            Ok(path)
        }
        "build" => {
            let mut built = None;
            run_job(ui, "Building firmware", "Firmware built", || {
                firmware::build().map(|(path, lines)| {
                    built = Some(path);
                    lines
                })
            });
            Ok(built)
        }
        "blocked" => {
            ui.warn("Install the Xtensa toolchain first: cargo install espup && espup install");
            Ok(None)
        }
        _ => Ok(None),
    }
}

/// Fetch the release list and let the user pick a version.
fn download_release(ui: &mut Ui) -> Result<Option<PathBuf>, Cancelled> {
    let mut spinner = ui.spinner();
    spinner.start("Fetching the list of firmware versions");
    let releases = firmware::fetch_releases();
    let releases = match releases {
        Ok(r) if r.is_empty() => {
            spinner.error("No firmware releases published yet");
            ui.info("Build from source instead, or cut a release first.");
            return Ok(None);
        }
        Ok(r) => {
            spinner.stop(&format!("Found {} version(s)", r.len()));
            r
        }
        Err(e) => {
            spinner.error("Could not fetch the release list");
            for line in e.lines() {
                ui.error(line);
            }
            return Ok(None);
        }
    };

    let default = firmware::pick_release(&releases, None).ok().map(|r| r.tag.clone());
    let options: Vec<SelectOption<Option<Release>>> = releases
        .iter()
        .map(|r| {
            let mut hint = match r.blocker() {
                Some(why) => why.to_string(),
                None => format!("{} KiB", r.asset_size / 1024),
            };
            if Some(&r.tag) == default.as_ref() {
                hint.push_str("  (recommended)");
            }
            SelectOption::new(
                r.blocker().is_none().then(|| r.clone()),
                r.label(),
            )
            .with_hint(hint)
        })
        .chain(std::iter::once(SelectOption::new(None, "Back")))
        .collect();

    let Some(release) = ui.select("Which version?", &options)? else {
        return Ok(None);
    };

    let mut path = None;
    run_job(ui, &format!("Downloading {}", release.tag), "Firmware downloaded", || {
        firmware::download_release(&release).map(|(p, lines)| {
            path = Some(p);
            lines
        })
    });
    Ok(path)
}

/// The firmware submenu.
fn firmware_menu(ui: &mut Ui, snapshot: &Snapshot) -> Result<(), Cancelled> {
    let ready = snapshot.usb == UsbMode::Bootloader;
    let options = vec![
        SelectOption::new("get", "Get firmware").with_hint(snapshot.firmware_hint()),
        SelectOption::new("flash", "Flash the device").with_hint(if snapshot.image.is_none() {
            "no image yet".to_string()
        } else if ready {
            "ready".to_string()
        } else {
            "will reboot into bootloader mode first".to_string()
        }),
        SelectOption::new("backup", "Back up the stock firmware")
            .with_hint("optional — keeps your exact current image"),
        SelectOption::new("restore", "Restore the stock firmware")
            .with_hint(if flash::backup_path().is_file() {
                "download Work Louder's, or use your backup"
            } else {
                "downloads Work Louder's published firmware"
            }),
        SelectOption::new("back", "Back"),
    ];

    match ui.select("Firmware", &options)? {
        "get" => {
            obtain_firmware(ui)?;
        }
        "flash" => {
            if snapshot.image.is_none() {
                ui.warn("Get a firmware image first.");
                return Ok(());
            }
            if !ensure_bootloader(ui)? {
                return Ok(());
            }
            if ui.confirm("Write firmware to the device now?", true)?
                && port_is_clear(ui)?
                && run_job(ui, "Flashing", "Firmware written", || {
                    flash::flash_capture(None, None)
                })
            {
    // The write deliberately leaves the device in the bootloader: entering it
    // set the force-download bit, which survives a reset, so a device that
    // simply rebooted here would come straight back to download mode and never
    // run the firmware that was just written. Clearing the bit and resetting is
    // what actually starts it.
                run_job(ui, "Starting the new firmware", "Device restarted", || {
                    wldevice::exit_bootloader(None)
                });
            }
        }
        "backup" => {
            if !ensure_bootloader(ui)? || !port_is_clear(ui)? {
                return Ok(());
            }
            run_job(ui, "Reading the stock firmware", "Stock firmware saved", || {
                flash::backup(None, None).map(|(_, lines)| lines)
            });
        }
        "restore" => restore_stock(ui)?,
        _ => {}
    }
    Ok(())
}

/// Put the vendor's firmware back on the device.
///
/// Work Louder publish unencrypted merged images to a public GitHub repo, so
/// going back no longer depends on having taken a backup first — that was the
/// single scariest thing about installing OpenMicro. A backup, if one exists,
/// is still offered because it is the user's *exact* image including their
/// settings, which a stock release is not.
fn restore_stock(ui: &mut Ui) -> Result<(), Cancelled> {
    let backup = flash::backup_path();
    let mut options = vec![SelectOption::new(
        "official",
        "Download Work Louder's firmware",
    )
    .with_hint("published releases — no backup needed")];
    if backup.is_file() {
        options.push(
            SelectOption::new("backup", "Use my own backup")
                .with_hint(short_path(&backup)),
        );
    }
    options.push(SelectOption::new("back", "Back"));

    let image = match ui.select("Which stock firmware?", &options)? {
        "official" => match download_stock(ui)? {
            Some(path) => path,
            None => return Ok(()),
        },
        "backup" => backup,
        _ => return Ok(()),
    };

    ui.note(
        "This overwrites OpenMicro",
        &format!(
            "The device will be returned to the vendor firmware and will stop \
             talking to OpenMicro.\n\nWriting: {}",
            short_path(&image)
        ),
    );
    if !ui.confirm("Overwrite the device with this firmware?", false)? {
        return Ok(());
    }
    if !ensure_bootloader(ui)? || !port_is_clear(ui)? {
        return Ok(());
    }
    let image_for_job = image.clone();
    if run_job(ui, "Restoring the stock firmware", "Stock firmware restored", move || {
        flash::restore(Some(&image_for_job), None)
    }) {
        run_job(ui, "Starting the vendor firmware", "Device restarted", || {
            wldevice::exit_bootloader(None)
        });
    }
    Ok(())
}

/// Fetch the vendor's published stock firmware, letting the user pick a version.
fn download_stock(ui: &mut Ui) -> Result<Option<PathBuf>, Cancelled> {
    let mut spinner = ui.spinner();
    spinner.start("Fetching Work Louder's firmware releases");
    let releases = match firmware::fetch_stock_releases() {
        Ok(r) if r.is_empty() => {
            spinner.error("No vendor firmware releases found");
            return Ok(None);
        }
        Ok(r) => {
            spinner.stop(&format!("Found {} vendor release(s)", r.len()));
            r
        }
        Err(e) => {
            spinner.error("Could not fetch the vendor release list");
            for line in e.lines() {
                ui.error(line);
            }
            return Ok(None);
        }
    };

    let options: Vec<SelectOption<Option<Release>>> = releases
        .iter()
        .map(|r| {
            let hint = match r.blocker() {
                Some(_) => "no merged image in this release".to_string(),
                None => format!("{} KiB", r.asset_size / 1024),
            };
            SelectOption::new(r.blocker().is_none().then(|| r.clone()), r.label())
                .with_hint(hint)
        })
        .chain(std::iter::once(SelectOption::new(None, "Back")))
        .collect();

    let Some(release) = ui.select("Which vendor version?", &options)? else {
        return Ok(None);
    };

    let mut path = None;
    run_job(ui, &format!("Downloading {}", release.tag), "Vendor firmware downloaded", || {
        firmware::download_stock_release(&release).map(|(p, lines)| {
            path = Some(p);
            lines
        })
    });
    Ok(path)
}

/// Warn when another process will fight us for the serial port.
///
/// Worth interrupting for: ModemManager opening the port mid-transfer kills a
/// 4 MiB backup partway through with an error that reads like a hardware fault.
/// Returns false when the user would rather stop and fix it first.
fn port_is_clear(ui: &mut Ui) -> Result<bool, Cancelled> {
    let Some(warning) = flash::port_contention() else {
        return Ok(true);
    };
    ui.note("Something else is using the serial port", &warning);
    ui.confirm("Try anyway?", false)
}

/// Put the device into bootloader mode if it is not already, asking first.
fn ensure_bootloader(ui: &mut Ui) -> Result<bool, Cancelled> {
    match wldevice::usb_mode() {
        UsbMode::Bootloader => Ok(true),
        UsbMode::Absent => {
            ui.error("No macropad on USB. Connect it with a data-capable cable.");
            Ok(false)
        }
        UsbMode::App(_) => {
            if !ui.confirm("Reboot the device into bootloader mode?", true)? {
                return Ok(false);
            }
            Ok(run_job(
                ui,
                "Rebooting into bootloader mode",
                "In bootloader mode",
                wldevice::enter_bootloader,
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Agents
// ---------------------------------------------------------------------------

/// Detect coding agents and wire up the ones the user picks.
fn agents_flow(ui: &mut Ui) -> Result<(), Cancelled> {
    let home = agents::home();
    let rows = agents::detect(&home);

    if agents::hook_binary().is_none() {
        ui.warn("openmicro-hook is not on PATH — hooks will install but stay inert until it is.");
    }

    let installable: Vec<_> = rows
        .iter()
        .filter(|r| !matches!(r.status, HookStatus::Blocked(_)))
        .collect();
    if installable.is_empty() {
        ui.warn("No supported coding agents found on this computer.");
        return Ok(());
    }

    let already: Vec<String> = rows
        .iter()
        .filter(|r| r.status == HookStatus::Installed)
        .map(|r| r.kind.label().to_string())
        .collect();

    let items: Vec<Item> = installable
        .iter()
        .filter(|r| r.status != HookStatus::Installed)
        .map(|r| {
            Item::new(r.kind.slug(), r.kind.label())
                .with_hint(short_path(&r.config_path))
                .with_detail(if r.present {
                    format!(
                        "Detected on this computer. Its hooks will be merged into {}, \
                         keeping everything already in there.",
                        short_path(&r.config_path)
                    )
                } else {
                    "Not detected here — install it anyway if you plan to use it.".to_string()
                })
        })
        .collect();

    if items.is_empty() {
        ui.success("Every detected coding agent is already wired up.");
        return Ok(());
    }

    let preselected: Vec<String> = installable
        .iter()
        .filter(|r| r.present && r.status == HookStatus::Missing)
        .map(|r| r.kind.slug().to_string())
        .collect();

    let opts = MultiOpts {
        max_visible: 8,
        locked: (!already.is_empty()).then(|| crate::prompt::LockedSection {
            title: "Already wired up".to_string(),
            items: already,
            max_shown: 6,
        }),
        list_title: Some("Available agents".to_string()),
        searchable: items.len() > 6,
        show_detail: true,
        detail_lines: 2,
        required: false,
        initial_selected: preselected,
    };

    let chosen = ui.search_multiselect("Which agents should drive the macropad?", &items, &opts)?;
    if chosen.is_empty() {
        ui.info("No agents selected.");
        return Ok(());
    }

    let kinds: Vec<AgentKind> = chosen.iter().filter_map(|v| AgentKind::from_slug(v)).collect();
    run_job(ui, "Installing hooks", "Hooks installed", || {
        install_agents(&kinds, &home)
    });
    Ok(())
}

/// Install several agents, reporting every outcome.
///
/// One failure does not abort the rest — the user picked several and deserves
/// the ones that worked — but the overall result is an error so the UI can
/// never show a clean success for a partial run.
fn install_agents(kinds: &[AgentKind], home: &std::path::Path) -> Result<Vec<String>, String> {
    let mut lines = Vec::new();
    let mut failed = false;
    for kind in kinds {
        match agents::install(*kind, home) {
            Ok(report) => lines.push(report.summary()),
            Err(e) => {
                failed = true;
                lines.push(format!("{}: FAILED — {e}", kind.slug()));
            }
        }
    }
    if failed {
        Err(lines.join("\n"))
    } else {
        Ok(lines)
    }
}

// ---------------------------------------------------------------------------
// Daemon and device
// ---------------------------------------------------------------------------

fn daemon_menu(ui: &mut Ui, snapshot: &Snapshot) -> Result<(), Cancelled> {
    let running = snapshot.daemon.running;
    let options = vec![
        SelectOption::new("toggle", if running { "Stop it" } else { "Start it" })
            .with_hint(snapshot.daemon.describe()),
        SelectOption::new("restart", "Restart it"),
        SelectOption::new(
            "enable",
            if snapshot.daemon.enabled { "Stop starting it at login" } else { "Start it at login" },
        ),
        SelectOption::new("back", "Back"),
    ];

    match ui.select("Background service", &options)? {
        "toggle" if running => {
            run_job(ui, "Stopping the service", "Service stopped", daemon::stop);
        }
        "toggle" => {
            run_job(ui, "Starting the service", "Service running", daemon::start);
        }
        "restart" => {
            run_job(ui, "Restarting the service", "Service restarted", daemon::restart);
        }
        "enable" if snapshot.daemon.enabled => {
            run_job(ui, "Disabling the service", "Service disabled", daemon::disable);
        }
        "enable" => {
            run_job(ui, "Enabling the service", "Service enabled", daemon::enable);
        }
        _ => {}
    }
    Ok(())
}

fn device_menu(ui: &mut Ui, snapshot: &Snapshot) -> Result<(), Cancelled> {
    let options = vec![
        SelectOption::new("check", "Check what's connected")
            .with_hint("scans USB and Bluetooth"),
        SelectOption::new("boot", "Reboot into bootloader mode")
            .with_hint(match snapshot.usb {
                UsbMode::Bootloader => "already there",
                UsbMode::Absent => "device not connected",
                UsbMode::App(_) => "sends sys.bootloader over USB",
            }),
        SelectOption::new("exit", "Leave bootloader mode").with_hint(match snapshot.usb {
            UsbMode::Bootloader => "clears the download-boot flag and resets",
            _ => "not in bootloader mode",
        }),
        SelectOption::new("back", "Back"),
    ];

    match ui.select("Device", &options)? {
        "check" => {
            let mut spinner = ui.spinner();
            spinner.start("Scanning USB and Bluetooth");
            let seen = probe::probe();
            spinner.stop("Scan complete");
            ui.note("Device", &describe_probe(&seen));
        }
        "boot" => {
            run_job(
                ui,
                "Rebooting into bootloader mode",
                "In bootloader mode",
                wldevice::enter_bootloader,
            );
        }
        "exit" => {
            run_job(ui, "Leaving bootloader mode", "Device restarted", || {
                wldevice::exit_bootloader(None)
            });
        }
        _ => {}
    }
    Ok(())
}

/// Multi-line description of a probe result, for the note box.
fn describe_probe(seen: &probe::Probe) -> String {
    let usb = match seen.usb {
        flash::DeviceState::Bootloader => "in bootloader mode, ready to flash",
        flash::DeviceState::NormalDevice => "connected, running its firmware",
        flash::DeviceState::Absent => "not connected",
    };
    let ble = match seen.ble {
        probe::BleState::OpenMicro => "advertising the OpenMicro service",
        probe::BleState::StockLike => "advertising, looks like stock firmware",
        probe::BleState::Absent => "nothing found",
        probe::BleState::Unavailable => "no Bluetooth adapter available",
    };
    let firmware = match seen.firmware() {
        probe::FirmwareKind::OpenMicro => "OpenMicro",
        probe::FirmwareKind::Stock => "stock (vendor)",
        probe::FirmwareKind::Bootloader => "none — ROM bootloader",
        probe::FirmwareKind::Unknown => "unknown",
    };
    let reach = match seen.connection() {
        probe::Connection::Cable => "over USB",
        probe::Connection::Ble => "over Bluetooth only",
        probe::Connection::None => "not reachable",
    };
    let summary = if seen.any_device() {
        format!("reachable {reach}")
    } else {
        "no macropad found".to_string()
    };
    format!("{summary}\n\nUSB:       {usb}\nBluetooth: {ble}\nFirmware:  {firmware}")
}

// ---------------------------------------------------------------------------
// Live view
// ---------------------------------------------------------------------------

/// Build the live-view frame. Pure, so the layout is testable without a
/// daemon, a terminal, or a device.
fn live_frame(
    style: &crate::prompt::Style,
    snap: &client::SnapshotDto,
    connected: bool,
    bar: &str,
) -> Vec<String> {
    let mut lines = Vec::new();
    let status = if connected {
        style.green("● connected")
    } else {
        style.yellow("○ waiting for the service")
    };
    let battery = match snap.battery {
        Some(pct) => format!("  battery {pct}%{}", if snap.charging { " (charging)" } else { "" }),
        None => String::new(),
    };
    lines.push(format!("{bar}  {status}{}", style.dim(&battery)));
    lines.push(bar.to_string());

    if snap.sessions.is_empty() {
        lines.push(format!("{bar}  {}", style.dim("no agent sessions yet")));
    } else {
        let owner = snap.owner.clone().unwrap_or_default();
        for session in &snap.sessions {
            let id = format!("{}:{}", session.agent, session.session);
            let slot = session.slot.map(|s| format!("key {s}")).unwrap_or_else(|| "—".into());
            let row = format!("{:<8}  {:<18}  {}", slot, session.agent, session.state);
            // The focused session is the one the macropad is actually showing.
            if id == owner {
                lines.push(format!("{bar}  {} {}", style.green("▸"), style.bold(&row)));
            } else {
                lines.push(format!("{bar}    {}", style.dim(&row)));
            }
        }
    }
    lines.push(bar.to_string());
    lines.push(format!("{bar}  {}", style.dim("press any key to go back")));
    lines
}

/// Watch agent state change in place until a key is pressed.
///
/// This is the one screen that is not a prompt: it redraws on a timer rather
/// than on input. It uses the same frame machinery as the prompts, so it
/// collapses into the transcript the same way when it exits.
fn live_status(ui: &mut Ui) -> Result<(), Cancelled> {
    if !daemon::is_running() {
        ui.warn("The background service isn't running, so there is nothing to watch yet.");
        return Ok(());
    }

    let snap = std::sync::Arc::new(std::sync::Mutex::new(client::SnapshotDto::default()));
    let connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    spawn_snapshot_reader(snap.clone(), connected.clone());

    let style = ui.style();
    let bar = style.dim(ui.symbols().bar);
    let mut columns = ui.columns();

    let result = (|| -> std::io::Result<()> {
        let _raw = crate::prompt::term::RawGuard::new(true)?;
        let mut zone = crate::prompt::term::FrameZone::new();
        let mut out = std::io::stdout();
        loop {
            {
                let snap = snap.lock().unwrap();
                let live = connected.load(std::sync::atomic::Ordering::Relaxed);
                zone.draw(&mut out, &live_frame(&style, &snap, live, &bar), columns)?;
            }
            if crossterm::event::poll(std::time::Duration::from_millis(250))? {
                match crate::prompt::term::next_event()? {
                    crate::prompt::term::PromptEvent::Key(_) => break,
                    crate::prompt::term::PromptEvent::Resize(c) => {
                        columns = c;
                        zone.resize(c);
                    }
                }
            }
        }
        Ok(())
    })();

    if let Err(e) = result {
        ui.error(&format!("live view stopped: {e}"));
    }
    Ok(())
}

/// Follow the daemon's control socket in the background, keeping `snap` fresh.
fn spawn_snapshot_reader(
    snap: std::sync::Arc<std::sync::Mutex<client::SnapshotDto>>,
    connected: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::io::BufRead;
    use std::sync::atomic::Ordering;

    std::thread::spawn(move || {
        let path = daemon::socket_path();
        loop {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(stream) => {
                    connected.store(true, Ordering::Relaxed);
                    for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
                        if let Some(parsed) = client::parse_snapshot(&line) {
                            *snap.lock().unwrap() = parsed;
                        }
                    }
                    connected.store(false, Ordering::Relaxed);
                }
                Err(_) => connected.store(false, Ordering::Relaxed),
            }
            std::thread::sleep(std::time::Duration::from_millis(500));
        }
    });
}

// ---------------------------------------------------------------------------
// Settings
// ---------------------------------------------------------------------------

/// Brightness, per-state colours and the idle-sleep timeout.
///
/// Every change is sent to the daemon, which applies it live and persists it,
/// so there is no separate save step.
fn settings_menu(ui: &mut Ui) -> Result<(), Cancelled> {
    if !daemon::is_running() {
        ui.warn("The background service isn't running, so settings can't be applied.");
        return Ok(());
    }

    // Seed the hints from the daemon's live config so the menu shows what the
    // settings currently are, not just what they could be.
    let current = read_current_settings();
    let options = vec![
        SelectOption::new("brightness", "Brightness").with_hint(match &current {
            Some(c) => format!("currently {}%", percent(c.brightness)),
            None => "unknown".to_string(),
        }),
        SelectOption::new("colour", "Colour for an agent").with_hint(match &current {
            Some(c) => format!("Claude is currently {}", colour_name(&c.colors.claude)),
            None => "unknown".to_string(),
        }),
        SelectOption::new("sleep", "Idle timeout before the lights sleep").with_hint(
            match &current {
                Some(c) if c.sleep_minutes == 0 => "currently never".to_string(),
                Some(c) => format!("currently {} minute(s)", c.sleep_minutes),
                None => "unknown".to_string(),
            },
        ),
        SelectOption::new("back", "Back"),
    ];

    match ui.select("Lights and sleep", &options)? {
        "brightness" => {
            let levels: Vec<SelectOption<u8>> = [16u8, 64, 128, 192, 255]
                .into_iter()
                .map(|v| {
                    SelectOption::new(v, format!("{}%", percent(v)))
                        .with_hint(format!("value {v}"))
                })
                .collect();
            let value = ui.select("How bright?", &levels)?;
            send_command(ui, &openmicro_proto::Command::SetBrightness(value), "Brightness set");
        }
        "colour" => {
            // Colour identifies *which agent*, not what it is doing — the
            // effect and the underglow motion carry the state. So this picks a
            // colour per agent, and the hint shows what it is now.
            let agents: Vec<SelectOption<openmicro_proto::AgentKind>> = [
                openmicro_proto::AgentKind::Claude,
                openmicro_proto::AgentKind::Codex,
                openmicro_proto::AgentKind::Grok,
                openmicro_proto::AgentKind::Other,
            ]
            .into_iter()
            .map(|kind| {
                let label = match kind {
                    openmicro_proto::AgentKind::Other => "Any other agent".to_string(),
                    _ => kind.label().to_string(),
                };
                let opt = SelectOption::new(kind, label);
                match &current {
                    Some(c) => opt.with_hint(colour_name(&c.colors.for_kind(kind))),
                    None => opt,
                }
            })
            .collect();
            let agent = ui.select("Which agent?", &agents)?;

            let colours: Vec<SelectOption<openmicro_proto::Rgb>> = client::PALETTE
                .iter()
                .map(|rgb| {
                    SelectOption::new(*rgb, colour_name(rgb))
                        .with_hint(format!("{},{},{}", rgb.r, rgb.g, rgb.b))
                })
                .collect();
            let rgb = ui.select("Which colour?", &colours)?;
            send_command(
                ui,
                &openmicro_proto::Command::SetAgentColor { agent, rgb },
                "Colour set",
            );
        }
        "sleep" => {
            let choices: Vec<SelectOption<u32>> = [0u32, 1, 3, 5, 15, 60]
                .into_iter()
                .map(|m| {
                    let label = if m == 0 {
                        "Never sleep".to_string()
                    } else {
                        format!("After {m} minute(s)")
                    };
                    SelectOption::new(m, label)
                })
                .collect();
            let minutes = ui.select("When should the lights sleep?", &choices)?;
            send_command(
                ui,
                &openmicro_proto::Command::SetSleepMinutes(minutes),
                "Idle timeout set",
            );
        }
        _ => {}
    }
    Ok(())
}

/// Brightness as a percentage of the 0..=255 range, for display.
fn percent(brightness: u8) -> u16 {
    brightness as u16 * 100 / 255
}

/// Read one snapshot from the daemon, for the settings menu's current values.
///
/// Returns `None` rather than an error: not knowing the current brightness is
/// not worth interrupting the user over, and the menu says "unknown".
fn read_current_settings() -> Option<client::SnapshotDto> {
    use std::io::BufRead;
    let stream = std::os::unix::net::UnixStream::connect(daemon::socket_path()).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line).ok()?;
    client::parse_snapshot(&line)
}

/// Human name for one of the preset colours.
fn colour_name(rgb: &openmicro_proto::Rgb) -> String {
    match (rgb.r, rgb.g, rgb.b) {
        (255, 255, 255) => "White".into(),
        (255, 0, 0) => "Red".into(),
        (0, 200, 80) => "Green".into(),
        (40, 90, 255) => "Blue".into(),
        (255, 140, 0) => "Orange".into(),
        (160, 60, 255) => "Purple".into(),
        (r, g, b) => format!("{r},{g},{b}"),
    }
}

/// Send one command to the daemon over the control socket.
fn send_command(ui: &mut Ui, command: &openmicro_proto::Command, done: &str) {
    use std::io::Write;

    let result = (|| -> Result<(), String> {
        let mut stream = std::os::unix::net::UnixStream::connect(daemon::socket_path())
            .map_err(|e| format!("cannot reach the service: {e}"))?;
        let mut line =
            serde_json::to_string(command).map_err(|e| format!("cannot encode command: {e}"))?;
        line.push('\n');
        stream
            .write_all(line.as_bytes())
            .map_err(|e| format!("cannot send command: {e}"))
    })();

    match result {
        Ok(()) => ui.success(done),
        Err(e) => ui.error(&e),
    }
}

// ---------------------------------------------------------------------------
// Uninstall
// ---------------------------------------------------------------------------

/// Remove OpenMicro from the machine, one informed choice at a time.
fn uninstall_flow(ui: &mut Ui) -> Result<(), Cancelled> {
    let home = agents::home();
    let found = uninstall::survey(&home);
    let present: Vec<_> = found.iter().filter(|f| f.present).collect();

    if present.is_empty() {
        ui.info("Nothing of OpenMicro's is installed on this computer.");
        return Ok(());
    }

    let items: Vec<Item> = present
        .iter()
        .map(|f| {
            let detail = if f.target.is_irreversible() {
                format!(
                    "Cannot be re-created — it is a dump of this device. {} \
                     (Work Louder's own firmware can still be downloaded.)",
                    f.detail
                )
            } else {
                format!("{} Reinstalling OpenMicro restores this.", f.detail)
            };
            Item::new(f.target.id(), f.target.label())
                .with_hint(f.detail.clone())
                .with_detail(detail)
        })
        .collect();

    let opts = MultiOpts {
        max_visible: 8,
        list_title: Some("Found on this computer".to_string()),
        searchable: false,
        show_detail: true,
        detail_lines: 2,
        required: true,
        initial_selected: present
            .iter()
            .filter(|f| f.target.default_selected())
            .map(|f| f.target.id().to_string())
            .collect(),
        ..MultiOpts::default()
    };

    let chosen = ui.search_multiselect("What should be removed?", &items, &opts)?;
    if chosen.is_empty() {
        ui.info("Nothing selected.");
        return Ok(());
    }

    let mut targets: Vec<Target> = chosen.iter().filter_map(|id| Target::from_id(id)).collect();

    // The backup gets its own question. It is the one thing here that cannot be
    // undone by reinstalling, so a single "remove these?" is not enough consent.
    if targets.contains(&Target::StockBackup) {
        ui.warn(
            "You chose to delete the stock-firmware backup. It is the only copy of this \
             device's exact original image; Work Louder's published firmware can still be \
             downloaded, but your own snapshot cannot be re-created.",
        );
        if !ui.confirm("Delete the stock-firmware backup as well?", false)? {
            targets.retain(|t| *t != Target::StockBackup);
            ui.info("Keeping the backup; removing everything else.");
        }
    }
    if targets.is_empty() {
        ui.info("Nothing left to remove.");
        return Ok(());
    }

    // Name the actual paths: "Settings" is not enough to consent to deleting a
    // directory, and the user cannot see what the label covers otherwise.
    let manifest = targets
        .iter()
        .map(|t| {
            let paths = found
                .iter()
                .find(|f| f.target == *t)
                .map(|f| {
                    f.paths.iter().map(|p| short_path(p)).collect::<Vec<_>>().join(", ")
                })
                .unwrap_or_default();
            if paths.is_empty() {
                format!("• {}", t.label())
            } else {
                format!("• {}\n    {paths}", t.label())
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    ui.note("About to remove", &manifest);
    if !ui.confirm("Remove these now?", false)? {
        ui.info("Nothing was removed.");
        return Ok(());
    }

    let results = uninstall::remove(&targets, &home);
    for result in &results {
        for line in &result.lines {
            ui.info(line);
        }
        if let Some(e) = &result.error {
            ui.error(&format!("{}: {e}", result.target.label()));
        }
    }
    let summary = uninstall::summarise(&results);
    if results.iter().any(|r| r.error.is_some()) {
        ui.error(&summary);
    } else {
        ui.success(&summary);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_wordmark_is_a_rectangle() {
        let widths: Vec<usize> = WORDMARK.iter().map(|r| r.chars().count()).collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "ragged wordmark rows: {widths:?}"
        );
        assert_eq!(widths[0], 74);
        assert!(
            widths[0] < WORDMARK_MIN_COLUMNS,
            "the width gate must leave room for the wordmark itself"
        );
    }

    #[test]
    fn short_path_abbreviates_the_home_directory() {
        let home = agents::home();
        let inside = home.join(".cache/openmicro/x.bin");
        assert_eq!(short_path(&inside), "~/.cache/openmicro/x.bin");
        // A path outside HOME is shown in full rather than mangled.
        assert_eq!(short_path(std::path::Path::new("/etc/hosts")), "/etc/hosts");
    }

    #[test]
    fn snapshot_hints_describe_every_device_state() {
        let base = Snapshot {
            usb: UsbMode::Absent,
            daemon: daemon::Status { running: false, unit_installed: false, enabled: false },
            agents_missing: 0,
            agents_installed: 0,
            image: None,
            cached_version: None,
        };
        assert!(base.device_hint().contains("not connected"));

        let boot = Snapshot { usb: UsbMode::Bootloader, ..base.clone() };
        assert!(boot.device_hint().contains("bootloader"));

        let app = Snapshot { usb: UsbMode::App(wldevice::CODEX_MICRO_PID), ..base.clone() };
        assert!(app.device_hint().contains("Codex Micro"), "{}", app.device_hint());
    }

    #[test]
    fn agent_hints_distinguish_wired_from_missing() {
        let base = Snapshot {
            usb: UsbMode::Absent,
            daemon: daemon::Status { running: false, unit_installed: false, enabled: false },
            agents_missing: 0,
            agents_installed: 0,
            image: None,
            cached_version: None,
        };
        assert!(base.agents_hint().contains("no supported agents"));
        assert!(Snapshot { agents_installed: 2, ..base.clone() }.agents_hint().contains("2 wired up"));
        assert!(Snapshot { agents_missing: 3, ..base.clone() }
            .agents_hint()
            .contains("none wired up"));
        let mixed = Snapshot { agents_installed: 1, agents_missing: 2, ..base.clone() };
        assert!(mixed.agents_hint().contains("1 wired up"));
        assert!(mixed.agents_hint().contains("2 still missing"));
    }

    #[test]
    fn firmware_hint_prefers_the_downloaded_version_over_the_path() {
        let base = Snapshot {
            usb: UsbMode::Absent,
            daemon: daemon::Status { running: false, unit_installed: false, enabled: false },
            agents_missing: 0,
            agents_installed: 0,
            image: None,
            cached_version: None,
        };
        assert!(base.firmware_hint().contains("no image"));

        let with_path = Snapshot { image: Some(PathBuf::from("/tmp/fw.bin")), ..base.clone() };
        assert!(with_path.firmware_hint().contains("/tmp/fw.bin"));

        let with_version = Snapshot {
            image: Some(PathBuf::from("/tmp/fw.bin")),
            cached_version: Some("v1.2.0".to_string()),
            ..base
        };
        assert!(with_version.firmware_hint().contains("v1.2.0"));
    }

    #[test]
    fn probe_descriptions_name_all_three_facts() {
        let seen = probe::Probe {
            usb: flash::DeviceState::Bootloader,
            ble: probe::BleState::Unavailable,
        };
        let text = describe_probe(&seen);
        assert!(text.contains("USB:") && text.contains("Bluetooth:") && text.contains("Firmware:"));
        assert!(text.contains("ready to flash"));
        assert!(text.contains("no Bluetooth adapter"));
    }

    #[test]
    fn installing_no_agents_is_not_a_failure() {
        let dir = std::env::temp_dir().join(format!("openmicro-app-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(install_agents(&[], &dir).unwrap(), Vec::<String>::new());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

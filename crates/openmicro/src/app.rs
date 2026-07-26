use std::path::PathBuf;

use crate::agents::{self, AgentKind, HookStatus};
use crate::daemon;
use crate::display;
use crate::firmware::{self, Release};
use crate::flash;
use crate::client;
use crate::probe;
use crate::prompt::{Cancelled, Item, MultiOpts, SelectOption, Ui};
use crate::uninstall::{self, Target};
use crate::wldevice::{self, UsbMode};

pub const WORDMARK: [&str; 6] = [
    " ██████╗ ██████╗ ███████╗███╗   ██╗███╗   ███╗██╗ ██████╗██████╗  ██████╗ ",
    "██╔═══██╗██╔══██╗██╔════╝████╗  ██║████╗ ████║██║██╔════╝██╔══██╗██╔═══██╗",
    "██║   ██║██████╔╝█████╗  ██╔██╗ ██║██╔████╔██║██║██║     ██████╔╝██║   ██║",
    "██║   ██║██╔═══╝ ██╔══╝  ██║╚██╗██║██║╚██╔╝██║██║██║     ██╔══██╗██║   ██║",
    "╚██████╔╝██║     ███████╗██║ ╚████║██║ ╚═╝ ██║██║╚██████╗██║  ██║╚██████╔╝",
    " ╚═════╝ ╚═╝     ╚══════╝╚═╝  ╚═══╝╚═╝     ╚═╝╚═╝ ╚═════╝╚═╝  ╚═╝ ╚═════╝ ",
];

const WORDMARK_MIN_COLUMNS: usize = 78;

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
            (Some(p), Some(v)) if *p == firmware::cache_image() => format!("{v} downloaded"),
            (Some(p), _) => format!("image at {}", short_path(p)),
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

fn short_path(path: &std::path::Path) -> String {
    let home = agents::home();
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

pub fn run() -> anyhow::Result<()> {
    let mut ui = Ui::new();
    if ui.columns() >= WORDMARK_MIN_COLUMNS {
        ui.banner(&WORDMARK);
    }
    let badge = ui.style().bg_cyan(&ui.style().fg256(0, " openmicro "));
    ui.intro(&badge);

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
            ui.warn("Cancelled — back to the menu.");
        }
    }

    ui.outro("Bye.");
    Ok(())
}

fn main_menu(ui: &mut Ui, snapshot: &Snapshot) -> Result<Action, Cancelled> {
    let options = vec![
        SelectOption::new(Action::Setup, "Set up my macropad")
            .with_hint("firmware, then coding agents — start here"),
        SelectOption::new(Action::Live, "Watch agent activity").with_hint(
            if snapshot.daemon.running { "live view" } else { "needs the service running" },
        ),
        SelectOption::new(Action::Settings, "Lights and sleep").with_hint(
            if snapshot.daemon.running {
                "brightness, per-agent colours, idle timeout"
            } else {
                "needs the service running"
            },
        ),
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

fn guided_setup(ui: &mut Ui) -> Result<(), Cancelled> {
    ui.note(
        "Before you start",
        "OpenMicro replaces the firmware on your macropad with its own build. The\n\
         vendor does not support this, and it may void your warranty.\n\
         \n\
         To go back, use Firmware > Restore the stock firmware.",
    );
    if !ui.confirm("Understood — continue?", false)? {
        return Ok(());
    }

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
            ui.error("No macropad on USB. Connect it with a data-capable cable.");
            return Ok(());
        }
        (UsbMode::Bootloader, _) => spinner.stop("Found it, already in bootloader mode"),
        (UsbMode::App(pid), _) => {
            spinner.stop(&format!("Found {}", wldevice::product_name(pid)));
        }
    }

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

    if obtain_firmware(ui)?.is_none() {
        return Ok(());
    }
    if !ui.confirm("Write this firmware to the device now?", true)? || !port_is_clear(ui)? {
        return Ok(());
    }
    if !run_job(ui, "Flashing", "Firmware written", || flash::flash_capture(None, None)) {
        return Ok(());
    }
    start_firmware(ui);

    finish_setup(ui)
}

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

fn firmware_source(ui: &mut Ui) -> Result<Option<PathBuf>, Cancelled> {
    let sources = firmware::Sources::detect();
    let mut options = Vec::new();
    if sources.can_download() {
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
        "blocked" => Ok(None),
        _ => Ok(None),
    }
}

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
            SelectOption::new(Some(r.clone()), r.label()).with_hint(hint)
        })
        .chain(std::iter::once(SelectOption::new(None, "Back")))
        .collect();

    let Some(release) = ui.select("Which version?", &options)? else {
        return Ok(None);
    };
    if let Some(why) = release.blocker() {
        ui.warn(why);
        return Ok(None);
    }

    let mut path = None;
    run_job(ui, &format!("Downloading {}", release.tag), "Firmware downloaded", || {
        firmware::download_release(&release).map(|(p, lines)| {
            path = Some(p);
            lines
        })
    });
    Ok(path)
}

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
        SelectOption::new("restore", "Restore the stock firmware")
            .with_hint("downloads Work Louder's published firmware"),
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
                start_firmware(ui);
            }
        }
        "restore" => restore_stock(ui)?,
        _ => {}
    }
    Ok(())
}

fn restore_stock(ui: &mut Ui) -> Result<(), Cancelled> {
    let Some(image) = download_stock(ui)? else {
        return Ok(());
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
        flash::restore(&image_for_job, None)
    }) {
        run_job(ui, "Starting the vendor firmware", "Device restarted", || {
            wldevice::exit_bootloader(None)
        });
    }
    Ok(())
}

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
            SelectOption::new(Some(r.clone()), r.label()).with_hint(hint)
        })
        .chain(std::iter::once(SelectOption::new(None, "Back")))
        .collect();

    let Some(release) = ui.select("Which vendor version?", &options)? else {
        return Ok(None);
    };
    if let Some(why) = release.blocker() {
        ui.warn(why);
        return Ok(None);
    }

    let mut path = None;
    run_job(ui, &format!("Downloading {}", release.tag), "Vendor firmware downloaded", || {
        firmware::download_stock_release(&release).map(|(p, lines)| {
            path = Some(p);
            lines
        })
    });
    Ok(path)
}

fn port_is_clear(ui: &mut Ui) -> Result<bool, Cancelled> {
    let Some(warning) = flash::port_contention() else {
        return Ok(true);
    };
    ui.note("Something else is using the serial port", &warning);
    ui.confirm("Try anyway?", false)
}

fn start_firmware(ui: &mut Ui) {
    run_job(ui, "Starting the new firmware", "Device restarted", || {
        wldevice::exit_bootloader(None)
    });
}

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
                        "Detected here. Merged into {}; your existing settings are kept.",
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

fn daemon_menu(ui: &mut Ui, snapshot: &Snapshot) -> Result<(), Cancelled> {
    let running = snapshot.daemon.running;
    let options = vec![
        SelectOption::new("toggle", if running { "Stop it" } else { "Start it" })
            .with_hint(snapshot.daemon.describe()),
        SelectOption::new("restart", "Restart it"),
        SelectOption::new(
            "enable",
            if snapshot.daemon.enabled {
                "Stop starting it at login"
            } else {
                "Start it at login (starts it now too)"
            },
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
                UsbMode::App(_) => "asks the firmware to reboot",
            }),
        SelectOption::new("exit", "Leave bootloader mode").with_hint(match snapshot.usb {
            UsbMode::Bootloader => "restarts it into its firmware",
            _ => "not in bootloader mode",
        }),
        SelectOption::new("lights", "What the lights are doing")
            .with_hint(match display::find_port() {
                Some(_) => "demo, identify, or back to normal",
                None => "needs a USB cable",
            }),
        SelectOption::new("back", "Back"),
    ];

    match ui.select("Device", &options)? {
        "lights" => {
            let modes = vec![
                SelectOption::new(display::Mode::Demo, "Demo")
                    .with_hint("walk every state the board can show"),
                SelectOption::new(display::Mode::Identify, "Identify LEDs")
                    .with_hint("one LED at a time, for mapping the chain order"),
                SelectOption::new(display::Mode::Normal, "Normal")
                    .with_hint("back to showing real agent state"),
            ];
            let mode = ui.select("What should the lights do?", &modes)?;
            run_job(ui, "Switching the lights", &format!("{} mode", mode.label()), || {
                display::send(mode)
            });
        }
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

fn live_status(ui: &mut Ui) -> Result<(), Cancelled> {
    if !daemon::is_running() {
        ui.warn("The background service isn't running, so there is nothing to watch yet.");
        return Ok(());
    }

    let snap = std::sync::Arc::new(std::sync::Mutex::new(client::SnapshotDto::default()));
    let connected = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    spawn_snapshot_reader(snap.clone(), connected.clone(), stop.clone());

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

    stop.store(true, std::sync::atomic::Ordering::Relaxed);
    if let Err(e) = result {
        ui.error(&format!("live view stopped: {e}"));
    }
    Ok(())
}

fn spawn_snapshot_reader(
    snap: std::sync::Arc<std::sync::Mutex<client::SnapshotDto>>,
    connected: std::sync::Arc<std::sync::atomic::AtomicBool>,
    stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use std::io::BufRead;
    use std::sync::atomic::Ordering;

    std::thread::spawn(move || {
        let path = daemon::socket_path();
        while !stop.load(Ordering::Relaxed) {
            match std::os::unix::net::UnixStream::connect(&path) {
                Ok(stream) => {
                    connected.store(true, Ordering::Relaxed);
                    for line in std::io::BufReader::new(stream).lines().map_while(Result::ok) {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
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

fn settings_menu(ui: &mut Ui) -> Result<(), Cancelled> {
    if !daemon::is_running() {
        ui.warn("The background service isn't running, so settings can't be applied.");
        return Ok(());
    }

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
            let agents: Vec<SelectOption<openmicro_proto::AgentKind>> = [
                openmicro_proto::AgentKind::Claude,
                openmicro_proto::AgentKind::Codex,
                openmicro_proto::AgentKind::Grok,
                openmicro_proto::AgentKind::Opencode,
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

fn percent(brightness: u8) -> u16 {
    brightness as u16 * 100 / 255
}

fn read_current_settings() -> Option<client::SnapshotDto> {
    use std::io::BufRead;
    let stream = std::os::unix::net::UnixStream::connect(daemon::socket_path()).ok()?;
    let mut line = String::new();
    std::io::BufReader::new(stream).read_line(&mut line).ok()?;
    client::parse_snapshot(&line)
}

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
            Item::new(f.target.id(), f.target.label())
                .with_hint(f.detail.clone())
                .with_detail("Reinstalling OpenMicro restores this.".to_string())
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

    let targets: Vec<Target> = chosen.iter().filter_map(|id| Target::from_id(id)).collect();
    if targets.is_empty() {
        ui.info("Nothing left to remove.");
        return Ok(());
    }

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

        let downloaded = Snapshot {
            image: Some(firmware::cache_image()),
            cached_version: Some("v1.2.0".to_string()),
            ..base.clone()
        };
        assert!(downloaded.firmware_hint().contains("v1.2.0"));

        let source_build = Snapshot {
            image: Some(PathBuf::from("/tmp/fw.bin")),
            cached_version: Some("v1.2.0".to_string()),
            ..base
        };
        let hint = source_build.firmware_hint();
        assert!(hint.contains("/tmp/fw.bin"), "{hint}");
        assert!(!hint.contains("v1.2.0"), "a source build is not the downloaded release: {hint}");
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

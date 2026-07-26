# Troubleshooting

## Flashing dies partway through

Symptom: esptool stops with `Packet content transfer stopped`, or a timeout, and
it looks like a hardware fault. It usually is not.

ModemManager probes every new `/dev/ttyACM*` device it sees, and on this board
that is the very port the transfer is streaming over. It opens the port mid-write
and the transfer dies.

Stop it for the session:

```sh
sudo systemctl stop ModemManager
```

Or exclude the device permanently, which survives reboots and does not require
stopping a service you may want for other hardware:

```sh
echo 'ATTRS{idVendor}=="303a", ENV{ID_MM_DEVICE_IGNORE}="1"' \
  | sudo tee /etc/udev/rules.d/99-openmicro.rules
sudo udevadm control --reload
```

The TUI checks for a running ModemManager before flashing and warns, but it
cannot tell whether it will actually interfere — hence the offer to continue.

## The device is not detected

- Use a data-capable USB cable. Charge-only cables enumerate nothing at all, so
  the device looks absent rather than broken.
- In bootloader mode the device appears as `303a:1001`. Running its firmware it
  is `303a:8297`, `303a:8298` or `303a:8360`.
- `303a:1001` is ambiguous: it is both the ESP32-S3 ROM bootloader *and* any
  firmware exposing a USB-Serial-JTAG console, which OpenMicro's does for its
  logs. The TUI settles it by asking the device to identify itself — the ROM
  bootloader never answers.

## The device is stuck in bootloader mode

Entering the bootloader sets a force-download bit that survives a reset, so a
device that simply reboots comes straight back to download mode. Clearing the bit
is what actually starts the firmware, and **Device → Leave bootloader mode** does
it.

By hand:

```sh
esptool --chip esp32s3 --before usb-reset --after watchdog-reset \
  write-mem 0x6000812C 0
```

## The lights are stuck in demo mode

Older firmware took bare single letters as serial commands. Because a tty echoes
received bytes back by default, the firmware's own log output arrived back at it
as input — and any line containing a `d` selected demo mode. The board
effectively reprogrammed itself.

Current firmware requires a `!` prefix (`!n`, `!d`, `!i`, `!?`) and drops
unprefixed bytes silently, which makes the loop impossible. If you see this,
you are running an old build: reflash.

## Settings changes do nothing

Settings are applied by the daemon, so it has to be running. The menu says so in
the hint when it is not.

If `config.toml` fails to parse, the daemon logs the error and runs on defaults —
and refuses to write over the broken file, so your settings are still there once
the syntax error is fixed. Check the daemon's output:

```sh
systemctl --user status openmicrod.service
```

## The macropad never lights up, and nothing reports an error

The adapters are deliberately silent: `openmicro-hook` always exits 0 and
no-ops when the daemon is down, so it can never block or fail an agent. That
also means a misconfiguration is quiet.

Check, in order:

1. The daemon is running: `systemctl --user status openmicrod.service`.
2. `openmicro-hook` is on the `PATH` of the process running the agent. The TUI
   warns about this when wiring agents up.
3. The agent's hooks are actually installed — **Coding agents** in the menu shows
   the status of each.
4. The daemon and the hook agree on where the socket is. They both use
   `$XDG_RUNTIME_DIR`, falling back to the system temp directory.

## Mode switching says the service could not be paused

Switching the lights needs the serial port to itself, so the TUI stops the daemon
for the write and starts it again afterwards. That is done through systemd, so it
only works for a daemon systemd started. A daemon you launched by hand has to be
stopped by hand.

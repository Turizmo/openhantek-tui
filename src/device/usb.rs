//! Find and open the real scope with libusb, and load its firmware.

use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use rusb::{Device, GlobalContext, UsbContext};

use super::firmware;
use super::protocol::*;
use super::transport::UsbTransport;

/// How long to wait for the scope to come back after the firmware upload.
/// It takes about 3 s; VMs and slow hubs can take longer.
pub const RENUMERATE_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Found {
    /// OpenHantek firmware is running.
    Ready,
    /// The scope needs (new) firmware.
    NeedsFirmware,
}

/// What kind of device this is, from its USB IDs.
pub fn classify(vid: u16, pid: u16, bcd: u16) -> Option<Found> {
    match (vid, pid) {
        (VID_FIRMWARE, PID_6022BE) if bcd >= FIRMWARE_VERSION => Some(Found::Ready),
        (VID_FIRMWARE, PID_6022BE) => Some(Found::NeedsFirmware),
        (VID_LOADER, PID_6022BE) | (VID_LOADER, PID_DSO2020) => Some(Found::NeedsFirmware),
        _ => None,
    }
}

fn bcd(dev: &Device<GlobalContext>) -> Option<(u16, u16, u16)> {
    let d = dev.device_descriptor().ok()?;
    // rusb decodes the BCD digits; put them back together.
    let v = d.device_version();
    let major = v.major() as u16;
    let bcd = ((major / 10) << 12)
        | ((major % 10) << 8)
        | ((v.minor() as u16) << 4)
        | v.sub_minor() as u16;
    Some((d.vendor_id(), d.product_id(), bcd))
}

/// Find the first scope on the bus.
fn find() -> Result<Option<(Device<GlobalContext>, Found)>> {
    let list = rusb::GlobalContext::default()
        .devices()
        .context("cannot list USB devices")?;
    let mut needs_fw = None;
    for dev in list.iter() {
        let Some((vid, pid, bcd)) = bcd(&dev) else {
            continue;
        };
        match classify(vid, pid, bcd) {
            Some(Found::Ready) => return Ok(Some((dev, Found::Ready))),
            Some(Found::NeedsFirmware) if needs_fw.is_none() => needs_fw = Some(dev),
            _ => {}
        }
    }
    Ok(needs_fw.map(|d| (d, Found::NeedsFirmware)))
}

/// Turn a libusb error into a message that says what to do.
pub fn explain(err: rusb::Error, what: &str) -> anyhow::Error {
    match err {
        rusb::Error::Access => anyhow!(
            "{what}: permission denied. Add udev rules for the scope \
             (NixOS: services.udev.packages = [ pkgs.openhantek6022 ]; other systems: \
             copy 60-openhantek.rules to /etc/udev/rules.d/), then replug it"
        ),
        rusb::Error::Busy => anyhow!(
            "{what}: the scope is in use by another program. Close OpenHantek6022 \
             (or any other scope program) and try again"
        ),
        rusb::Error::NoDevice => anyhow!("{what}: the scope was unplugged"),
        e => anyhow::Error::new(e).context(what.to_string()),
    }
}

/// Load the firmware into a scope that needs it, then wait for it to come back.
fn load_firmware(dev: &Device<GlobalContext>, firmware: Option<&Path>) -> Result<()> {
    let (path, records) = firmware::load(firmware)?;
    let handle = dev
        .open()
        .map_err(|e| explain(e, "cannot open the scope to load firmware"))?;
    // Not supported everywhere; it does not matter for this device.
    let _ = handle.set_auto_detach_kernel_driver(true);
    handle
        .claim_interface(INTERFACE)
        .map_err(|e| explain(e, "cannot claim the scope to load firmware"))?;
    let mut t = UsbTransport::new(handle, Duration::from_millis(1000));
    firmware::upload(&mut t, &records)
        .with_context(|| format!("firmware upload from {} failed", path.display()))?;
    let _ = t.handle.release_interface(INTERFACE);
    Ok(())
}

/// Wait for the scope with firmware. Polls by USB IDs, not by bus address:
/// the address changes when the scope comes back.
fn wait_ready(timeout: Duration) -> Result<Device<GlobalContext>> {
    let t0 = Instant::now();
    // Give the old device time to drop off the bus first.
    std::thread::sleep(Duration::from_millis(500));
    while t0.elapsed() < timeout {
        if let Some((dev, Found::Ready)) = find()? {
            // A short pause so udev can set the permissions.
            std::thread::sleep(Duration::from_millis(300));
            return Ok(dev);
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    bail!(
        "the scope did not come back {} s after the firmware upload. Unplug it and try again",
        timeout.as_secs()
    )
}

/// The opened scope.
pub struct Opened {
    pub transport: UsbTransport,
    pub serial: String,
    pub bcd: u16,
    /// True when we loaded the firmware just now.
    pub loaded_firmware: bool,
}

/// Find the scope, load firmware if needed, open and claim it.
/// `status` gets short progress texts.
pub fn open(firmware: Option<&Path>, status: &mut dyn FnMut(&str)) -> Result<Opened> {
    let Some((mut dev, found)) = find()? else {
        bail!("no DSO-6022BE found. Check the USB cable: lsusb must show 04b4:6022 or 04b5:6022");
    };
    let mut loaded_firmware = false;
    if found == Found::NeedsFirmware {
        status("Loading firmware into the scope...");
        load_firmware(&dev, firmware)?;
        status("Waiting for the scope to restart...");
        dev = wait_ready(RENUMERATE_TIMEOUT)?;
        loaded_firmware = true;
    }
    let (_, _, bcd) = bcd(&dev).ok_or_else(|| anyhow!("cannot read the device descriptor"))?;
    let handle = dev
        .open()
        .map_err(|e| explain(e, "cannot open the scope"))?;
    let serial = dev
        .device_descriptor()
        .ok()
        .and_then(|d| handle.read_serial_number_string_ascii(&d).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown".to_string());
    handle
        .claim_interface(INTERFACE)
        .map_err(|e| explain(e, "cannot claim the scope"))?;
    Ok(Opened {
        transport: UsbTransport::new(handle, Duration::from_millis(500)),
        serial,
        bcd,
        loaded_firmware,
    })
}

/// The OpenHantek6022 calibration file for this scope, if there is one.
pub fn openhantek_ini(serial: &str) -> Option<String> {
    let home = std::env::var_os("HOME")?;
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|d| !d.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from(home).join(".config"));
    let path = base
        .join("OpenHantek")
        .join(format!("DSO-6022BE_{serial}_calibration.ini"));
    std::fs::read_to_string(path).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_ids() {
        assert_eq!(classify(0x04B5, 0x6022, 0x0210), Some(Found::Ready));
        assert_eq!(classify(0x04B5, 0x6022, 0x0211), Some(Found::Ready));
        assert_eq!(classify(0x04B5, 0x6022, 0x0209), Some(Found::NeedsFirmware));
        assert_eq!(classify(0x04B4, 0x6022, 0x0000), Some(Found::NeedsFirmware));
        assert_eq!(classify(0x04B4, 0x2020, 0x0000), Some(Found::NeedsFirmware));
        assert_eq!(classify(0x04B4, 0x602A, 0x0000), None);
        assert_eq!(classify(0x1D6B, 0x0002, 0x0000), None);
    }

    #[test]
    fn explain_gives_hints() {
        let e = explain(rusb::Error::Access, "open");
        assert!(e.to_string().contains("udev"));
        let e = explain(rusb::Error::Busy, "claim");
        assert!(e.to_string().contains("OpenHantek6022"));
    }
}

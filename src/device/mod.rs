//! Talking to the DSO-6022BE over USB.

pub mod fake;
pub mod firmware;
pub mod hantek;
pub mod hex;
pub mod protocol;
pub mod stream;
pub mod transport;
pub mod usb;

use std::path::Path;

use anyhow::Result;

use hantek::Scope6022;
use transport::Transport;

/// The driver over any transport.
pub type Driver = Scope6022<Box<dyn Transport>>;

/// An opened scope, ready to capture.
pub struct Connection {
    pub driver: Driver,
    /// Short name for the title bar.
    pub name: String,
    /// Things to tell the user (calibration notes and such).
    pub notes: Vec<String>,
}

/// Open the real scope: find it, load firmware if needed, read calibration.
pub fn open_hardware(firmware: Option<&Path>, status: &mut dyn FnMut(&str)) -> Result<Connection> {
    let opened = usb::open(firmware, status)?;
    let serial = opened.serial.clone();
    let mut driver: Driver = Scope6022::new(Box::new(opened.transport), serial.clone());
    let ini = usb::openhantek_ini(&serial);
    let mut notes = driver.load_calibration(ini.as_deref());
    if opened.loaded_firmware {
        notes.insert(0, "Firmware loaded".to_string());
    }
    Ok(Connection {
        driver,
        name: format!("DSO-6022BE {serial} FW{:04X}", opened.bcd),
        notes,
    })
}

/// Open the fake scope with the demo signals.
pub fn open_demo() -> Connection {
    let mut driver: Driver = Scope6022::new(Box::new(fake::FakeScope::demo()), "DEMO");
    let notes = driver.load_calibration(None);
    Connection {
        driver,
        name: "DEMO (fake scope)".to_string(),
        notes,
    }
}

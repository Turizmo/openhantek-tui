//! The USB calls the scope driver needs. Real hardware uses libusb (rusb);
//! tests and demo mode use a fake that acts like the scope.

use std::time::Duration;

use anyhow::Result;

/// Vendor request to the device, host to device (bmRequestType 0x40).
pub const VENDOR_OUT: u8 = 0x40;
/// Vendor request to the device, device to host (bmRequestType 0xC0).
pub const VENDOR_IN: u8 = 0xC0;

pub trait Transport: Send {
    /// Vendor control transfer, host to device.
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()>;
    /// Vendor control transfer, device to host. Returns bytes read.
    fn control_in(&mut self, request: u8, value: u16, index: u16, buf: &mut [u8]) -> Result<usize>;
    /// Bulk read. Returns bytes read.
    fn bulk_in(&mut self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> Result<usize>;
}

/// libusb transport for an opened, claimed device.
pub struct UsbTransport {
    pub handle: rusb::DeviceHandle<rusb::GlobalContext>,
    pub timeout: Duration,
}

impl Transport for UsbTransport {
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        let n = self
            .handle
            .write_control(VENDOR_OUT, request, value, index, data, self.timeout)?;
        if n != data.len() {
            anyhow::bail!(
                "control request {request:#04x}: wrote {n} of {} bytes",
                data.len()
            );
        }
        Ok(())
    }

    fn control_in(&mut self, request: u8, value: u16, index: u16, buf: &mut [u8]) -> Result<usize> {
        Ok(self
            .handle
            .read_control(VENDOR_IN, request, value, index, buf, self.timeout)?)
    }

    fn bulk_in(&mut self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        Ok(self.handle.read_bulk(endpoint, buf, timeout)?)
    }
}

/// One recorded control transfer. Used by tests.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlCall {
    pub request_type: u8,
    pub request: u8,
    pub value: u16,
    pub index: u16,
    pub data: Vec<u8>,
}

/// A transport that records every control call and answers nothing.
#[derive(Default)]
pub struct Recorder {
    pub calls: Vec<ControlCall>,
}

impl Transport for Recorder {
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        self.calls.push(ControlCall {
            request_type: VENDOR_OUT,
            request,
            value,
            index,
            data: data.to_vec(),
        });
        Ok(())
    }

    fn control_in(&mut self, request: u8, value: u16, index: u16, buf: &mut [u8]) -> Result<usize> {
        self.calls.push(ControlCall {
            request_type: VENDOR_IN,
            request,
            value,
            index,
            data: vec![],
        });
        buf.fill(0);
        Ok(buf.len())
    }

    fn bulk_in(&mut self, _endpoint: u8, buf: &mut [u8], _timeout: Duration) -> Result<usize> {
        buf.fill(0x80);
        Ok(buf.len())
    }
}

impl<T: Transport + ?Sized> Transport for Box<T> {
    fn control_out(&mut self, request: u8, value: u16, index: u16, data: &[u8]) -> Result<()> {
        (**self).control_out(request, value, index, data)
    }

    fn control_in(&mut self, request: u8, value: u16, index: u16, buf: &mut [u8]) -> Result<usize> {
        (**self).control_in(request, value, index, buf)
    }

    fn bulk_in(&mut self, endpoint: u8, buf: &mut [u8], timeout: Duration) -> Result<usize> {
        (**self).bulk_in(endpoint, buf, timeout)
    }
}

/// True when the error means the scope is gone (unplugged).
pub fn is_gone(err: &anyhow::Error) -> bool {
    err.chain().any(|e| {
        matches!(
            e.downcast_ref::<rusb::Error>(),
            Some(rusb::Error::NoDevice) | Some(rusb::Error::Io) | Some(rusb::Error::Pipe)
        )
    })
}

/// True when the error is a USB timeout.
pub fn is_timeout(err: &anyhow::Error) -> bool {
    err.chain()
        .any(|e| matches!(e.downcast_ref::<rusb::Error>(), Some(rusb::Error::Timeout)))
}

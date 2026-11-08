//! Async bulk streaming with libusb: many transfers stay in flight, so the
//! host always has a buffer ready when the scope's 2 KB FIFO fills up.
//!
//! One synchronous read at a time leaves the pipe idle between reads; at
//! more than about 400 kS/s the FIFO overflows in that time. With 16 to 32
//! transfers queued, the stream is gapless up to 32 MB/s (16 MS/s with two
//! channels, 30 MS/s with one), measured on the real scope.
//!
//! A thread of its own runs the libusb event loop. The transfer callback
//! copies the data into the stream [`Ring`](crate::stream::ring::Ring)
//! with the time it came off the bus, and resubmits the transfer at once.
//! Nothing else runs there, so slow processing never stops the USB pipe.
//! When the processing falls so far behind that the ring is full, the
//! ring drops the data and counts it, never silently.

use std::ffi::c_void;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use anyhow::{anyhow, Result};
use libusb1_sys as ffi;
use libusb1_sys::constants::*;

use crate::stream::ring::{Failure, Writer, MARK_SHORT};

/// How to stream.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StreamSetup {
    pub endpoint: u8,
    /// Bytes per transfer, a multiple of the packet size.
    pub transfer: usize,
    /// Transfers kept in flight.
    pub transfers: usize,
}

/// One piece of a stream.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub data: Vec<u8>,
    /// Bytes dropped just before this chunk, because the consumer was too
    /// slow. Whole transfers, so a multiple of the packet size.
    pub lost: u64,
    /// When the transfer completed. A slow consumer sees the chunk later,
    /// but this time is still when the data came off the bus.
    pub at: Instant,
}

/// State the callback and the owner share, under one lock: the callback
/// checks `stop` and resubmits while holding it, and [`UsbStream::drop`]
/// sets `stop` and cancels while holding it, so a transfer is never
/// resubmitted after its cancel.
struct Ctl {
    stop: bool,
    /// An error was reported: resubmit nothing more.
    failed: bool,
    in_flight: usize,
}

struct Inner {
    ctl: Mutex<Ctl>,
    /// Signalled when `in_flight` drops to 0.
    idle: Condvar,
    ring: Writer,
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Ctl> {
        // Never panic in the callback: a poisoned lock still holds valid data.
        self.ctl.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Put the data of a finished transfer into the ring. A transfer that
    /// is shorter than asked or not whole frames is marked: data may be
    /// missing there.
    fn deliver(&self, data: &[u8], asked: usize) {
        let ch = self.ring.ring().channels();
        let flags = if data.len() < asked || !data.len().is_multiple_of(ch) {
            MARK_SHORT
        } else {
            0
        };
        if data.is_empty() && flags == 0 {
            return;
        }
        self.ring.write(data, Instant::now(), flags);
    }

    fn fail(&self, e: rusb::Error) {
        self.ring.fail(Failure::Usb(e));
    }
}

/// libusb error code to a rusb error.
fn error_of_code(code: i32) -> rusb::Error {
    match code {
        LIBUSB_ERROR_IO => rusb::Error::Io,
        LIBUSB_ERROR_INVALID_PARAM => rusb::Error::InvalidParam,
        LIBUSB_ERROR_ACCESS => rusb::Error::Access,
        LIBUSB_ERROR_NO_DEVICE => rusb::Error::NoDevice,
        LIBUSB_ERROR_NOT_FOUND => rusb::Error::NotFound,
        LIBUSB_ERROR_BUSY => rusb::Error::Busy,
        LIBUSB_ERROR_TIMEOUT => rusb::Error::Timeout,
        LIBUSB_ERROR_OVERFLOW => rusb::Error::Overflow,
        LIBUSB_ERROR_PIPE => rusb::Error::Pipe,
        LIBUSB_ERROR_INTERRUPTED => rusb::Error::Interrupted,
        LIBUSB_ERROR_NO_MEM => rusb::Error::NoMem,
        LIBUSB_ERROR_NOT_SUPPORTED => rusb::Error::NotSupported,
        _ => rusb::Error::Other,
    }
}

/// Transfer status (not completed, not cancelled) to a rusb error.
fn error_of_status(status: i32) -> rusb::Error {
    match status {
        LIBUSB_TRANSFER_TIMED_OUT => rusb::Error::Timeout,
        LIBUSB_TRANSFER_STALL => rusb::Error::Pipe,
        LIBUSB_TRANSFER_NO_DEVICE => rusb::Error::NoDevice,
        LIBUSB_TRANSFER_OVERFLOW => rusb::Error::Overflow,
        _ => rusb::Error::Io,
    }
}

extern "system" fn callback(t: *mut ffi::libusb_transfer) {
    // SAFETY: `user_data` points to the `Inner` of the stream that owns
    // this transfer. It lives until every transfer has called back (the
    // owner waits for `in_flight == 0` before freeing anything). The
    // buffer is valid for `actual_length` bytes while the transfer is not
    // in flight, which it is not during its callback.
    unsafe {
        let inner = &*((*t).user_data as *const Inner);
        let status = (*t).status;
        let mut ctl = inner.lock();
        ctl.in_flight -= 1;
        match status {
            LIBUSB_TRANSFER_COMPLETED if !ctl.stop && !ctl.failed => {
                let asked = (*t).length.max(0) as usize;
                let len = ((*t).actual_length.max(0) as usize).min(asked);
                inner.deliver(std::slice::from_raw_parts((*t).buffer, len), asked);
                let r = ffi::libusb_submit_transfer(t);
                if r == 0 {
                    ctl.in_flight += 1;
                } else {
                    ctl.failed = true;
                    inner.fail(error_of_code(r));
                }
            }
            LIBUSB_TRANSFER_COMPLETED | LIBUSB_TRANSFER_CANCELLED => {}
            s => {
                if !ctl.stop && !ctl.failed {
                    ctl.failed = true;
                    inner.fail(error_of_status(s));
                }
            }
        }
        if ctl.in_flight == 0 {
            inner.idle.notify_all();
        }
    }
}

/// Raw pointers that the event thread and the owner pass around.
struct Raw<T>(*mut T);
// SAFETY: libusb transfers, handles and contexts may be used from any
// thread; all access to our own shared state goes through `Inner`'s locks.
unsafe impl<T> Send for Raw<T> {}

/// A running stream. Dropping it cancels the transfers, waits for them and
/// frees them.
pub struct UsbStream {
    inner: Arc<Inner>,
    transfers: Vec<Raw<ffi::libusb_transfer>>,
    /// The transfer buffers (from `Box<[u8]>::into_raw`, with their
    /// length). Only raw pointers, so nothing claims them while libusb
    /// writes into them; freed only after all transfers called back.
    buffers: Vec<(Raw<u8>, usize)>,
    ctx: Raw<ffi::libusb_context>,
    /// Tells the event thread to end.
    quit: Arc<std::sync::atomic::AtomicBool>,
    events: Option<JoinHandle<()>>,
}

/// How long dropping a stream waits for the cancelled transfers.
const CANCEL_WAIT: Duration = Duration::from_secs(5);

impl UsbStream {
    /// Queue the transfers on `handle`; they write into `ring`. Start the
    /// scope (E3 [01]) only after this returns, so no data comes before a
    /// buffer is ready.
    ///
    /// # Safety
    /// `handle` must be an open libusb device handle with the interface
    /// claimed, and it must stay open until the stream is dropped.
    pub unsafe fn start(
        ctx: *mut ffi::libusb_context,
        handle: *mut ffi::libusb_device_handle,
        setup: StreamSetup,
        ring: Writer,
    ) -> Result<UsbStream> {
        let inner = Arc::new(Inner {
            ctl: Mutex::new(Ctl {
                stop: false,
                failed: false,
                in_flight: 0,
            }),
            idle: Condvar::new(),
            ring,
        });
        let quit = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let events = {
            let ctx = Raw(ctx);
            let quit = quit.clone();
            std::thread::Builder::new()
                .name("usb events".into())
                .spawn(move || {
                    let ctx = ctx;
                    let tv = libc::timeval {
                        tv_sec: 0,
                        tv_usec: 100_000,
                    };
                    while !quit.load(Ordering::Acquire) {
                        // SAFETY: a valid context; `tv` outlives the call.
                        unsafe {
                            ffi::libusb_handle_events_timeout_completed(
                                ctx.0,
                                &tv,
                                std::ptr::null_mut(),
                            );
                        }
                    }
                })?
        };
        let mut s = UsbStream {
            inner,
            transfers: Vec::new(),
            buffers: Vec::new(),
            ctx: Raw(ctx),
            quit,
            events: Some(events),
        };
        let user = Arc::as_ptr(&s.inner) as *mut c_void;
        for _ in 0..setup.transfers.max(1) {
            let t = ffi::libusb_alloc_transfer(0);
            if t.is_null() {
                return Err(anyhow!("cannot allocate a USB transfer"));
            }
            s.transfers.push(Raw(t));
            let len = setup.transfer;
            let buf = Box::into_raw(vec![0u8; len].into_boxed_slice()) as *mut u8;
            s.buffers.push((Raw(buf), len));
            ffi::libusb_fill_bulk_transfer(
                t,
                handle,
                setup.endpoint,
                buf,
                len as i32,
                callback,
                user,
                // No timeout: at slow rates a deep queue waits long for
                // its last transfer. A dead scope ends the transfers with
                // an error instead; a silent one shows as no data.
                0,
            );
            let mut ctl = s.inner.lock();
            let r = ffi::libusb_submit_transfer(t);
            if r != 0 {
                drop(ctl);
                let e = error_of_code(r);
                let hint = if e == rusb::Error::NoMem {
                    " (raise /sys/module/usbcore/parameters/usbfs_memory_mb)"
                } else {
                    ""
                };
                // Drop cancels and frees what is queued already.
                return Err(anyhow::Error::new(e).context(format!(
                    "cannot queue {} USB transfers of {} kB{hint}",
                    setup.transfers,
                    setup.transfer / 1024
                )));
            }
            ctl.in_flight += 1;
        }
        Ok(s)
    }

    /// Cancel all transfers and wait until they called back. True when
    /// they all did in time.
    fn cancel(&mut self) -> bool {
        let mut ctl = self.inner.lock();
        ctl.stop = true;
        for t in &self.transfers {
            // SAFETY: an allocated transfer. Cancelling one that is not in
            // flight just returns LIBUSB_ERROR_NOT_FOUND.
            unsafe {
                ffi::libusb_cancel_transfer(t.0);
            }
        }
        let deadline = Instant::now() + CANCEL_WAIT;
        while ctl.in_flight > 0 {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            ctl = self
                .inner
                .idle
                .wait_timeout(ctl, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }
}

impl Drop for UsbStream {
    fn drop(&mut self) {
        let done = self.cancel();
        self.quit.store(true, Ordering::Release);
        // SAFETY: a valid context. Wakes the event thread at once.
        unsafe { ffi::libusb_interrupt_event_handler(self.ctx.0) };
        if let Some(h) = self.events.take() {
            let _ = h.join();
        }
        if done {
            for t in self.transfers.drain(..) {
                // SAFETY: allocated by us and no longer in flight.
                unsafe { ffi::libusb_free_transfer(t.0) };
            }
            for (b, len) in self.buffers.drain(..) {
                // SAFETY: from `Box::into_raw` of a slice of `len` bytes,
                // and no transfer uses it any more.
                drop(unsafe { Box::from_raw(std::ptr::slice_from_raw_parts_mut(b.0, len)) });
            }
        } else {
            // Some transfer never called back. Freeing it or its buffer
            // could let libusb write into freed memory: leak them (the
            // buffers are raw pointers, so they stay) and the shared state
            // its callback would use.
            self.transfers.clear();
            self.buffers.clear();
            std::mem::forget(self.inner.clone());
        }
    }
}

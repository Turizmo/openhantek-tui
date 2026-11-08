//! The sample ring: one big buffer that the USB side writes and the
//! processing thread reads. The bytes are kept as USB sends them
//! (interleaved CH1, CH2, CH1, ... with two channels), and every byte has
//! an absolute position since the stream started, so the readers see one
//! continuous stream, never blocks.
//!
//! One writer and one reader at a time. The reader says how far back it
//! still needs data ([`Ring::keep_from`]); the writer never overwrites
//! that. When there is no room, the writer drops the whole transfer and
//! counts it ([`Ring::overrun`]): the processing thread sees the count and
//! starts the stream again, so a gap is never hidden.

use std::cell::UnsafeCell;
use std::collections::VecDeque;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant};

/// Size of the ring the scope streams into: 64 MiB, 2 s at the top USB
/// rate (32 MB/s). The longest capture (50 ms/div, 500 ms) is 16 MB.
pub const STREAM_RING: usize = 1 << 26;

/// Most transfer marks kept for the reader.
const MARKS_MAX: usize = 8192;

/// Flag of a [`Mark`]: the transfer was shorter than asked, or not a
/// whole number of frames.
pub const MARK_SHORT: u8 = 1;

/// The end of one USB transfer: what the ring held after it, and when it
/// came off the bus.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mark {
    /// Bytes written since the start, this transfer included.
    pub end: u64,
    pub at: Instant,
    pub flags: u8,
}

/// Why the stream ended by itself (reported by the USB side).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    Usb(rusb::Error),
}

pub struct Ring {
    buf: Box<[UnsafeCell<u8>]>,
    mask: u64,
    channels: AtomicUsize,
    /// Changes at each reset, so a writer of an old stream writes nothing.
    epoch: AtomicU64,
    /// Bytes written since the reset.
    write: AtomicU64,
    /// The reader needs the bytes from here on.
    keep: AtomicU64,
    /// Bytes dropped because there was no room.
    overrun: AtomicU64,
    /// Once a transfer was dropped, all later ones are too (the data after
    /// a gap is of no use; the stream starts again).
    full: AtomicBool,
    marks: Mutex<VecDeque<Mark>>,
    failure: Mutex<Option<Failure>>,
    wake: Mutex<()>,
    cv: Condvar,
}

// SAFETY: the bytes are shared by one writer and one reader. The writer
// only writes bytes past `write` and before `keep + size`; the reader only
// reads bytes from `keep` to `write`. `write` is stored (Release) after
// the bytes are copied and loaded (Acquire) before they are read; `keep`
// is stored (Release) after the reader is done with the bytes before it.
// So the two never touch the same byte at the same time.
unsafe impl Sync for Ring {}
unsafe impl Send for Ring {}

/// The writing end of a ring for one stream.
#[derive(Clone)]
pub struct Writer {
    ring: std::sync::Arc<Ring>,
    epoch: u64,
}

impl Writer {
    pub fn ring(&self) -> &Ring {
        &self.ring
    }

    /// Append the bytes of one transfer that came off the bus `at`.
    /// False when there was no room: the transfer was dropped and counted.
    pub fn write(&self, data: &[u8], at: Instant, flags: u8) -> bool {
        self.ring.write(self.epoch, data, at, flags)
    }

    /// The stream ended with an error.
    pub fn fail(&self, f: Failure) {
        let r = &self.ring;
        if r.epoch.load(Ordering::Acquire) != self.epoch {
            return;
        }
        let mut slot = r.failure.lock().unwrap_or_else(|e| e.into_inner());
        if slot.is_none() {
            *slot = Some(f);
        }
        drop(slot);
        r.notify();
    }

    /// True while this writer's stream is the ring's stream.
    pub fn current(&self) -> bool {
        self.ring.epoch.load(Ordering::Acquire) == self.epoch
    }
}

impl Ring {
    /// A ring of `size` bytes, a power of two (and so even: a frame of
    /// two channels never wraps in the middle).
    pub fn new(size: usize) -> Self {
        assert!(size.is_power_of_two() && size >= 2, "ring size {size}");
        // `vec![0; n]` asks the OS for zeroed pages, which it maps only
        // when they are used: cheap, also for 64 MiB.
        let buf: Vec<UnsafeCell<u8>> = {
            let v = vec![0u8; size];
            // SAFETY: UnsafeCell<u8> has the same layout as u8.
            let mut v = std::mem::ManuallyDrop::new(v);
            unsafe {
                Vec::from_raw_parts(v.as_mut_ptr() as *mut UnsafeCell<u8>, v.len(), v.capacity())
            }
        };
        Self {
            buf: buf.into_boxed_slice(),
            mask: size as u64 - 1,
            channels: AtomicUsize::new(1),
            epoch: AtomicU64::new(0),
            write: AtomicU64::new(0),
            keep: AtomicU64::new(0),
            overrun: AtomicU64::new(0),
            full: AtomicBool::new(false),
            marks: Mutex::new(VecDeque::new()),
            failure: Mutex::new(None),
            wake: Mutex::new(()),
            cv: Condvar::new(),
        }
    }

    pub fn size(&self) -> usize {
        self.buf.len()
    }

    /// Start a new, empty stream of `channels` interleaved channels and
    /// return its writer. Writers of earlier streams write nothing more.
    /// Call it only when no writer is busy writing (the stream stopped).
    pub fn reset(self: &std::sync::Arc<Self>, channels: u8) -> Writer {
        let epoch = self.epoch.fetch_add(1, Ordering::AcqRel) + 1;
        self.channels
            .store(channels.clamp(1, 2) as usize, Ordering::Release);
        self.write.store(0, Ordering::Release);
        self.keep.store(0, Ordering::Release);
        self.overrun.store(0, Ordering::Release);
        self.full.store(false, Ordering::Release);
        self.marks.lock().unwrap_or_else(|e| e.into_inner()).clear();
        *self.failure.lock().unwrap_or_else(|e| e.into_inner()) = None;
        Writer {
            ring: self.clone(),
            epoch,
        }
    }

    pub fn channels(&self) -> usize {
        self.channels.load(Ordering::Acquire)
    }

    fn write(&self, epoch: u64, data: &[u8], at: Instant, flags: u8) -> bool {
        if self.epoch.load(Ordering::Acquire) != epoch {
            return false;
        }
        let n = data.len() as u64;
        let w = self.write.load(Ordering::Relaxed);
        let keep = self.keep.load(Ordering::Acquire);
        if self.full.load(Ordering::Relaxed) || w + n - keep > self.buf.len() as u64 {
            self.full.store(true, Ordering::Release);
            self.overrun.fetch_add(n, Ordering::AcqRel);
            self.notify();
            return false;
        }
        let size = self.buf.len();
        let mut at_byte = (w & self.mask) as usize;
        let mut rest = data;
        while !rest.is_empty() {
            let k = rest.len().min(size - at_byte);
            // SAFETY: the bytes `w .. w + n` are past `write` and within
            // `keep + size` (checked above), so the reader does not use
            // them; `at_byte + k <= size`.
            unsafe {
                std::ptr::copy_nonoverlapping(rest.as_ptr(), self.buf[at_byte].get(), k);
            }
            rest = &rest[k..];
            at_byte = 0;
        }
        self.write.store(w + n, Ordering::Release);
        {
            let mut m = self.marks.lock().unwrap_or_else(|e| e.into_inner());
            if m.len() >= MARKS_MAX {
                m.pop_front();
            }
            m.push_back(Mark {
                end: w + n,
                at,
                flags,
            });
        }
        self.notify();
        true
    }

    fn notify(&self) {
        let _g = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        self.cv.notify_all();
    }

    /// Bytes written since the start of the stream.
    pub fn written(&self) -> u64 {
        self.write.load(Ordering::Acquire)
    }

    /// Whole frames (one sample of each channel) written so far.
    pub fn frames(&self) -> u64 {
        self.written() / self.channels() as u64
    }

    /// Bytes dropped for want of room.
    pub fn overrun(&self) -> u64 {
        self.overrun.load(Ordering::Acquire)
    }

    /// Bytes the writer can still write before it must drop data.
    pub fn room(&self) -> u64 {
        let used = self.written() - self.keep.load(Ordering::Acquire);
        self.buf.len() as u64 - used
    }

    /// The error that ended the stream, if one did.
    pub fn failure(&self) -> Option<Failure> {
        self.failure
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    /// The reader needs no frame before `frame` any more. Never moves back.
    pub fn keep_from(&self, frame: u64) {
        let b = (frame * self.channels() as u64).min(self.written());
        self.keep.fetch_max(b, Ordering::AcqRel);
    }

    /// The first frame the ring still holds for the reader.
    pub fn kept(&self) -> u64 {
        self.keep
            .load(Ordering::Acquire)
            .div_ceil(self.channels() as u64)
    }

    /// Take the transfer marks that came since the last call.
    pub fn take_marks(&self) -> Vec<Mark> {
        self.marks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .drain(..)
            .collect()
    }

    /// Wait up to `timeout` until more than `frames` frames are written,
    /// a transfer is dropped or the stream fails. True when one of them
    /// happened.
    pub fn wait(&self, frames: u64, timeout: Duration) -> bool {
        let ready = || {
            self.frames() > frames
                || self.overrun() > 0
                || self
                    .failure
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .is_some()
        };
        let deadline = Instant::now() + timeout;
        let mut g = self.wake.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if ready() {
                return true;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            g = self
                .cv
                .wait_timeout(g, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// Give the bytes of `frames` to `f`, as one or two slices (two when
    /// they wrap around the end of the buffer). The frames must be kept
    /// and written.
    pub fn read<R>(&self, frames: Range<u64>, f: impl FnOnce(&[u8], &[u8]) -> R) -> R {
        let ch = self.channels() as u64;
        self.read_bytes(frames.start * ch..frames.end * ch, f)
    }

    /// [`Ring::read`] by byte positions.
    pub fn read_bytes<R>(&self, bytes: Range<u64>, f: impl FnOnce(&[u8], &[u8]) -> R) -> R {
        let (a, b) = (bytes.start, bytes.end);
        assert!(
            a <= b && b <= self.written() && a >= self.keep.load(Ordering::Acquire),
            "ring read of bytes {bytes:?} outside the kept ones"
        );
        let size = self.buf.len();
        let start = (a & self.mask) as usize;
        let n = (b - a) as usize;
        let first = n.min(size - start);
        // SAFETY: the bytes `a .. b` are between `keep` and `write`, so
        // the writer does not touch them while the reader holds them (see
        // the `Sync` impl). UnsafeCell<u8> has the layout of u8.
        unsafe {
            let base = self.buf.as_ptr() as *const u8;
            let s1 = std::slice::from_raw_parts(base.add(start), first);
            let s2 = std::slice::from_raw_parts(base, n - first);
            f(s1, s2)
        }
    }

    /// The bytes of `frames` as one vector (tests and small reads).
    pub fn to_vec(&self, frames: Range<u64>) -> Vec<u8> {
        self.read(frames, |a, b| [a, b].concat())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn ring(size: usize, ch: u8) -> (Arc<Ring>, Writer) {
        let r = Arc::new(Ring::new(size));
        let w = r.reset(ch);
        (r, w)
    }

    #[test]
    fn wraps_and_reads_two_slices() {
        let (r, w) = ring(16, 1);
        let now = Instant::now();
        assert!(w.write(&(0..10).collect::<Vec<u8>>(), now, 0));
        r.keep_from(8);
        assert!(w.write(&(10..20).collect::<Vec<u8>>(), now, 0));
        assert_eq!(r.frames(), 20);
        let (n1, n2) = r.read(8..20, |a, b| (a.len(), b.len()));
        assert_eq!((n1, n2), (8, 4));
        assert_eq!(r.to_vec(8..20), (8..20).collect::<Vec<u8>>());
        let marks = r.take_marks();
        assert_eq!(marks.iter().map(|m| m.end).collect::<Vec<_>>(), [10, 20]);
        assert!(r.take_marks().is_empty());
    }

    #[test]
    fn two_channel_frames_stay_whole() {
        let (r, w) = ring(64, 2);
        let now = Instant::now();
        // Odd pieces: a frame is split between two writes.
        let mut next = 0u32;
        let mut keep = 0;
        for len in [1usize, 3, 5, 7, 2, 9, 11, 1, 13, 4] {
            let piece: Vec<u8> = (0..len)
                .map(|_| {
                    next += 1;
                    (next - 1) as u8
                })
                .collect();
            assert!(w.write(&piece, now, 0));
            let f = r.frames();
            // Frame k holds bytes 2k and 2k + 1, also across the wrap.
            let bytes = r.to_vec(keep..f);
            for (j, pair) in bytes.chunks_exact(2).enumerate() {
                let k = keep + j as u64;
                assert_eq!(pair, [(2 * k) as u8, (2 * k + 1) as u8]);
            }
            keep = f;
            r.keep_from(keep);
        }
        assert_eq!(r.frames(), 28);
        assert_eq!(r.written(), 56);
    }

    #[test]
    fn full_ring_drops_and_counts() {
        let (r, w) = ring(16, 2);
        let now = Instant::now();
        assert!(w.write(&[0; 12], now, 0));
        assert_eq!(r.room(), 4);
        // No room for 6 more: dropped, not written over.
        assert!(!w.write(&[1; 6], now, 0));
        assert_eq!(r.overrun(), 6);
        assert_eq!(r.written(), 12);
        // Once dropped, later transfers go too, even with room again.
        r.keep_from(6);
        assert!(!w.write(&[1; 2], now, 0));
        assert_eq!(r.overrun(), 8);
        assert!(r.wait(100, Duration::ZERO));
        // A reset starts over; the old writer writes nothing more.
        let w2 = r.reset(2);
        assert!(!w.write(&[1; 2], now, 0));
        assert!(w2.write(&[1; 2], now, 0));
        assert_eq!((r.overrun(), r.frames()), (0, 1));
    }

    #[test]
    fn wait_wakes_on_data_and_failure() {
        let (r, w) = ring(1024, 1);
        assert!(!r.wait(0, Duration::from_millis(5)));
        let w2 = w.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(20));
            w2.write(&[1, 2, 3], Instant::now(), 0);
        });
        assert!(r.wait(0, Duration::from_secs(5)));
        t.join().unwrap();
        w.fail(Failure::Usb(rusb::Error::Io));
        assert!(r.wait(3, Duration::ZERO));
        assert_eq!(r.failure(), Some(Failure::Usb(rusb::Error::Io)));
    }

    #[test]
    #[should_panic]
    fn reading_dropped_frames_panics() {
        let (r, w) = ring(16, 1);
        w.write(&[0; 10], Instant::now(), 0);
        r.keep_from(5);
        r.read(2..6, |_, _| ());
    }
}

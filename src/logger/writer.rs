//! The writer thread: takes records from the acquisition thread and
//! writes them to the log file. Buffers in memory, hands the data to the
//! OS a few times a second (so a killed app leaves a readable file up to
//! there, and the log view sees new data), and fsyncs every few seconds.
//! Stops by itself when the file reaches the size limit.

use std::fs::File;
use std::io::Write;
use std::os::unix::fs::FileExt;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use super::format::{ChunkHead, Header, CHUNK_HEADER, FLAG_GAP, OPEN};

/// Messages to the writer.
#[derive(Clone, Debug, PartialEq)]
pub enum Msg {
    /// Create the file and write the header.
    Begin(Header),
    /// Records from step `first` on, as f32 values in file order.
    Records { first: u64, data: Vec<f32> },
    /// Raw records (raw logs) from step `first` on: the u8 ADC values in
    /// file order.
    Raw { first: u64, data: Vec<u8> },
    /// Data was lost before the next records.
    Gap,
    /// Close the file.
    Stop,
}

/// Why a log ended.
#[derive(Clone, Debug, PartialEq)]
pub enum StopReason {
    User,
    Limit,
    Error(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum LogState {
    /// Started, waiting for the first data from the scope.
    Waiting,
    Recording,
    Done(StopReason),
}

/// What the UI shows about a running log. Shared between the threads.
#[derive(Clone, Debug)]
pub struct Status {
    pub state: LogState,
    pub path: PathBuf,
    /// Bytes in the file, header included.
    pub bytes: u64,
    /// Steps written (gaps not counted).
    pub records: u64,
    /// Gaps so far.
    pub gaps: u32,
    /// Size limit in bytes, 0 for none.
    pub limit: u64,
    /// When the log was started (wall clock), for the elapsed time.
    pub started: Instant,
    /// A sample hit the end of the ADC range in the last second.
    pub clipped: bool,
    pub header: Option<Header>,
}

pub type Shared = Arc<Mutex<Status>>;

pub fn shared(path: PathBuf, limit: u64) -> Shared {
    Arc::new(Mutex::new(Status {
        state: LogState::Waiting,
        path,
        bytes: 0,
        records: 0,
        gaps: 0,
        limit,
        started: Instant::now(),
        clipped: false,
        header: None,
    }))
}

/// How often buffered data goes to the OS.
pub const FLUSH_EVERY: Duration = Duration::from_millis(250);
/// How often the file is synced to the disk.
pub const SYNC_EVERY: Duration = Duration::from_secs(2);
/// Flush early when the buffer gets this big.
const BUFFER_MAX: usize = 4 << 20;

pub struct Writer {
    tx: Sender<Msg>,
    handle: Option<JoinHandle<()>>,
}

impl Writer {
    pub fn spawn(shared: Shared) -> Writer {
        let (tx, rx) = mpsc::channel();
        let handle = std::thread::Builder::new()
            .name("log writer".into())
            .spawn(move || run(rx, shared))
            .expect("start the log writer thread");
        Writer {
            tx,
            handle: Some(handle),
        }
    }

    pub fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
    }

    /// Close the file and wait for the thread.
    pub fn finish(mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Stop);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct W {
    shared: Shared,
    file: Option<File>,
    header: Option<Header>,
    /// Data not yet written to the file.
    buf: Vec<u8>,
    /// File size, buffer included.
    size: u64,
    /// File offset and first step of the open chunk.
    chunk: Option<(u64, u64)>,
    /// Step after the last record written.
    next: u64,
    gap: bool,
    records: u64,
    gaps: u32,
    last_flush: Instant,
    last_sync: Instant,
    done: bool,
}

fn run(rx: Receiver<Msg>, shared: Shared) {
    let mut w = W {
        shared,
        file: None,
        header: None,
        buf: Vec::new(),
        size: 0,
        chunk: None,
        next: 0,
        gap: false,
        records: 0,
        gaps: 0,
        last_flush: Instant::now(),
        last_sync: Instant::now(),
        done: false,
    };
    loop {
        let msg = match rx.recv_timeout(FLUSH_EVERY) {
            Ok(m) => Some(m),
            Err(RecvTimeoutError::Timeout) => None,
            Err(RecvTimeoutError::Disconnected) => Some(Msg::Stop),
        };
        let stop = matches!(msg, Some(Msg::Stop));
        if !w.done {
            let r = match msg {
                Some(m) => w.handle(m),
                None => Ok(()),
            };
            let r = r.and_then(|_| w.tick());
            if let Err(e) = r {
                w.end(StopReason::Error(format!("{e}")));
            }
        }
        if stop {
            if !w.done {
                w.end(StopReason::User);
            }
            return;
        }
    }
}

impl W {
    fn handle(&mut self, m: Msg) -> std::io::Result<()> {
        match m {
            Msg::Begin(h) => {
                let path = self.shared.lock().unwrap().path.clone();
                if let Some(dir) = path.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut f = File::options().write(true).create_new(true).open(&path)?;
                let bytes = h.encode();
                f.write_all(&bytes)?;
                self.size = bytes.len() as u64;
                self.file = Some(f);
                let mut s = self.shared.lock().unwrap();
                s.header = Some(h.clone());
                s.state = LogState::Recording;
                s.bytes = self.size;
                drop(s);
                self.header = Some(h);
            }
            Msg::Records { first, data } => {
                let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
                self.records(first, &bytes)?
            }
            Msg::Raw { first, data } => self.records(first, &data)?,
            Msg::Gap => {
                self.gap = true;
                self.gaps += 1;
            }
            Msg::Stop => {}
        }
        Ok(())
    }

    /// Write records (encoded, `record_bytes` each) from step `first` on.
    fn records(&mut self, first: u64, data: &[u8]) -> std::io::Result<()> {
        let Some(h) = &self.header else {
            return Ok(());
        };
        let size = h.record_bytes() as u64;
        let n = data.len() as u64 / size.max(1);
        if n == 0 {
            return Ok(());
        }
        let limit = self.shared.lock().unwrap().limit;
        let new_chunk = self.chunk.is_none() || self.gap || first != self.next;
        let head = if new_chunk { CHUNK_HEADER as u64 } else { 0 };
        let mut take = n;
        let mut full = false;
        if limit > 0 && self.size + head + n * size > limit {
            take = limit.saturating_sub(self.size + head) / size;
            full = true;
        }
        if take > 0 {
            if new_chunk {
                let gap = self.chunk.is_some() || self.gap;
                self.close_chunk()?;
                let head = ChunkHead {
                    flags: if gap { FLAG_GAP } else { 0 },
                    first,
                    count: OPEN,
                };
                self.chunk = Some((self.size, first));
                self.buf.extend_from_slice(&head.encode());
                self.size += CHUNK_HEADER as u64;
                self.gap = false;
            }
            self.buf
                .extend_from_slice(&data[..(take * size) as usize]);
            self.size += take * size;
            self.next = first + take;
            self.records += take;
        }
        if full {
            self.end(StopReason::Limit);
        } else if self.buf.len() >= BUFFER_MAX {
            self.flush()?;
        }
        Ok(())
    }

    /// Write the buffer to the file.
    fn flush(&mut self) -> std::io::Result<()> {
        if let Some(f) = &mut self.file {
            f.write_all(&self.buf)?;
        }
        self.buf.clear();
        self.last_flush = Instant::now();
        let mut s = self.shared.lock().unwrap();
        s.bytes = self.size;
        s.records = self.records;
        s.gaps = self.gaps;
        Ok(())
    }

    /// Write the record count into the open chunk's head.
    fn close_chunk(&mut self) -> std::io::Result<()> {
        if let Some((at, first)) = self.chunk.take() {
            self.flush()?;
            if let Some(f) = &self.file {
                f.write_all_at(&(self.next - first).to_le_bytes(), at + 16)?;
            }
        }
        Ok(())
    }

    fn tick(&mut self) -> std::io::Result<()> {
        if self.last_flush.elapsed() >= FLUSH_EVERY {
            self.flush()?;
        }
        if self.last_sync.elapsed() >= SYNC_EVERY {
            if let Some(f) = &self.file {
                f.sync_data()?;
            }
            self.last_sync = Instant::now();
        }
        Ok(())
    }

    fn end(&mut self, why: StopReason) {
        let r = self.close_chunk().and_then(|_| {
            self.flush()?;
            match &self.file {
                Some(f) => f.sync_all(),
                None => Ok(()),
            }
        });
        let why = match (r, why) {
            (Err(e), StopReason::User | StopReason::Limit) => StopReason::Error(format!("{e}")),
            (_, w) => w,
        };
        self.done = true;
        self.file = None;
        self.shared.lock().unwrap().state = LogState::Done(why);
    }
}

/// Time of the first sample of a log that starts now with a piece of
/// `samples` samples at `rate`.
pub fn start_time(now: SystemTime, samples: usize, rate: f64) -> i64 {
    let back = Duration::from_secs_f64(samples as f64 / rate.max(1.0));
    let t = now.checked_sub(back).unwrap_or(now);
    t.duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as i64)
}

#[cfg(test)]
mod tests {
    use super::super::format::test_header;
    use super::super::reader::LogFile;
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("oht-writer-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        dir.join("a.ohlog")
    }

    #[test]
    fn writes_chunks_gaps_and_closes() {
        let path = temp("chunks");
        let sh = shared(path.clone(), 0);
        let w = Writer::spawn(sh.clone());
        let h = test_header([true, true], 1e-3);
        w.send(Msg::Begin(h.clone()));
        let rec = |v: f32| vec![v, v, v, -v, -v, -v];
        w.send(Msg::Records {
            first: 0,
            data: [rec(1.0), rec(2.0)].concat(),
        });
        w.send(Msg::Records {
            first: 2,
            data: rec(3.0),
        });
        w.send(Msg::Gap);
        w.send(Msg::Records {
            first: 10,
            data: rec(4.0),
        });
        w.finish();
        let s = sh.lock().unwrap().clone();
        assert_eq!(s.state, LogState::Done(StopReason::User));
        assert_eq!(s.records, 4);
        assert_eq!(s.gaps, 1);
        let len = std::fs::metadata(&path).unwrap().len();
        assert_eq!(s.bytes, len);
        let f = LogFile::open(&path).unwrap();
        assert_eq!(f.header, h);
        assert_eq!(f.end(), 11);
        assert_eq!(f.gaps(), vec![(3, 10)]);
        let r = f.read(0..11).unwrap();
        assert_eq!(r.len(), 4);
        assert_eq!(r[2].0, 2);
        assert_eq!(r[3].0, 10);
        assert_eq!(r[3].1[1], [-4.0; 3]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn stops_at_the_size_limit() {
        let path = temp("limit");
        let h = test_header([true, false], 1e-3);
        let head = h.encode().len() as u64;
        // Room for the chunk head and 10 records of 12 bytes.
        let limit = head + CHUNK_HEADER as u64 + 10 * 12 + 5;
        let sh = shared(path.clone(), limit);
        let w = Writer::spawn(sh.clone());
        w.send(Msg::Begin(h));
        w.send(Msg::Records {
            first: 0,
            data: vec![0.5; 3 * 25],
        });
        // Wait for the writer to stop by itself.
        let t0 = Instant::now();
        while !matches!(sh.lock().unwrap().state, LogState::Done(_)) {
            assert!(t0.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(sh.lock().unwrap().state, LogState::Done(StopReason::Limit));
        w.send(Msg::Records {
            first: 25,
            data: vec![0.5; 3],
        });
        w.finish();
        let len = std::fs::metadata(&path).unwrap().len();
        assert!(len <= limit);
        assert_eq!(len, limit - 5);
        assert_eq!(LogFile::open(&path).unwrap().end(), 10);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }

    #[test]
    fn a_killed_writer_leaves_a_readable_file() {
        // Like a crash: the open chunk never gets its count, and the last
        // record is cut in half.
        let path = temp("crash");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let h = test_header([true, false], 1e-3);
        let mut b = h.encode();
        b.extend_from_slice(
            &ChunkHead {
                flags: 0,
                first: 0,
                count: OPEN,
            }
            .encode(),
        );
        for v in [1.0f32, 2.0, 1.5, 3.0, 4.0, 3.5, 9.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
        std::fs::write(&path, &b).unwrap();
        let f = LogFile::open(&path).unwrap();
        assert_eq!(f.end(), 2);
        assert_eq!(f.read(0..2).unwrap()[1].1[0], [3.0, 4.0, 3.5]);
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
    }
}

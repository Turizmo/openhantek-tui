//! Acquisition: the processing thread. It talks to the scope, runs the
//! stream processing on the ring the USB side writes, and hands the
//! newest frame to the UI through [`Latest`]. The UI sends settings; each
//! hardware change gets a new generation number (`gen`, the stream
//! starts again) and each processing change another (`pgen`, only the
//! processing starts over), so the UI can drop frames made with old ones.
//!
//! Three threads: the USB events thread only copies transfers into the
//! ring; this thread does everything else; the UI takes finished frames.
//! A slow UI never makes this thread drop data: a frame the UI did not
//! take is replaced by a newer one.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, TryRecvError};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;

use crate::device::hantek::{Scope6022, SKIP_SAMPLES};
use crate::device::protocol::{AppCal, HwConfig, Rate};
use crate::device::transport::{is_gone, is_timeout, Transport};
use crate::device::Connection;
use crate::logger::binner::Binner;
use crate::logger::format::{ChannelInfo, Header};
use crate::logger::gapcheck::GapCheck;
use crate::logger::writer::{self, LogState, Msg, Shared, Writer};
use crate::stream::pipeline::{Pipeline, ProcSettings, Published};
use crate::stream::ring::{Failure, Ring, MARK_SHORT, STREAM_RING};

pub enum Command {
    /// Use these hardware settings from now on.
    Configure {
        gen: u64,
        cfg: HwConfig,
    },
    /// Use these processing settings from now on: the trigger search
    /// and the average start over, the stream goes on.
    Process {
        pgen: u64,
        p: Box<ProcSettings>,
    },
    /// Capture (true) or stop (false).
    Run(bool),
    /// Start the envelope logger. While it runs, the scope streams with
    /// the logger's settings, `Configure` waits until the log stops, and
    /// no frames go to the UI.
    StartLog(LogRequest),
    StopLog,
    Quit,
}

/// What to log.
#[derive(Clone, Debug)]
pub struct LogRequest {
    /// The writer reports here, and its `path` is the file to write.
    pub shared: Shared,
    /// The step in ns; 0 for a raw log (one ADC sample per step, stored
    /// as the raw byte).
    pub step_ns: u64,
    /// Channels to log.
    pub channels: [bool; 2],
    pub probe: [u32; 2],
    pub invert: [bool; 2],
    /// Volts/div at the probe tip (sets the gain, like in the scope view).
    pub volts_per_div: [f64; 2],
    pub gains: [usize; 2],
    pub dc: [bool; 2],
    pub cal_freq: u32,
    /// The max ADC rate setting (S/s with both channels).
    pub max_adc: f64,
}

impl LogRequest {
    /// Hardware settings: the ADC at the top rate `max_adc` allows, no
    /// oversampling, streaming; 1-channel mode when only CH1 is logged
    /// (twice the ADC rate), else both channels.
    pub fn hw_config(&self) -> (HwConfig, u32) {
        let channels = crate::logger::hw_channels(self.channels, self.max_adc);
        let (rate, n) = crate::logger::adc_for_step(self.step_ns, channels, self.max_adc);
        (
            HwConfig {
                rate,
                channels,
                gains: self.gains,
                dc: self.dc,
                cal_freq: self.cal_freq,
                roll: true,
            },
            n,
        )
    }
}

/// A running log, in the acquisition thread.
struct LogSession {
    req: LogRequest,
    cfg: HwConfig,
    /// Made at the first piece, with the calibration of the scope.
    binner: Option<Binner>,
    samples_per_step: u32,
    writer: Writer,
    /// Checks the stream against the clock; `None` until the first piece.
    check: Option<GapCheck>,
    /// The stream started again (after an error): flag a gap.
    restart: bool,
    clip_at: Option<Instant>,
}

impl LogSession {
    fn new(req: LogRequest) -> Self {
        let (cfg, n) = req.hw_config();
        let writer = Writer::spawn(req.shared.clone());
        Self {
            req,
            cfg,
            binner: None,
            samples_per_step: n,
            writer,
            check: None,
            restart: false,
            clip_at: None,
        }
    }

    fn header(&self, conn: &Connection, start_ns: i64) -> Header {
        let r = &self.req;
        let cal = &conn.driver.cal;
        let (cfg, n) = r.hw_config();
        let hs = cfg.high_speed();
        let ch = [0, 1].map(|c| {
            let g = r.gains[c];
            let zero = cal.raw(0.0, g, c, hs);
            ChannelInfo {
                enabled: r.channels[c],
                gain: g as u8,
                probe: r.probe[c].min(255) as u8,
                invert: r.invert[c],
                dc: r.dc[c],
                volts_per_div: r.volts_per_div[c] as f32,
                zero_count: zero as f32,
                volts_per_count: (cal.volts(zero + 1.0, g, c, hs) - cal.volts(zero, g, c, hs))
                    as f32,
            }
        });
        let raw = r.step_ns == 0;
        Header {
            start_ns,
            // A raw log's step is the ADC sample time (rounded here; the
            // ADC rate gives it exactly).
            step_ns: if raw {
                (1e9 / cfg.rate.raw).round().max(1.0) as u64
            } else {
                r.step_ns
            },
            adc_rate: cfg.rate.raw,
            samples_per_step: n,
            channels: ch,
            device: conn.name.clone(),
            raw,
        }
    }

    /// The binner, with the calibration of `conn`.
    fn make_binner(&self, conn: &Connection) -> Binner {
        let r = &self.req;
        let hs = self.cfg.high_speed();
        let lin = [0, 1].map(|c| {
            let (a, b) = conn.driver.cal.linear(r.gains[c], c, hs);
            let k = r.probe[c] as f64 * if r.invert[c] { -1.0 } else { 1.0 };
            (a * k, b * k)
        });
        let logged = (0..2).filter(|&c| r.channels[c]).collect();
        Binner::new(
            self.samples_per_step,
            self.cfg.channels,
            logged,
            lin,
            r.step_ns == 0,
        )
    }

    /// True when the writer stopped by itself (size limit, error).
    fn writer_done(&self) -> bool {
        matches!(self.req.shared.lock().unwrap().state, LogState::Done(_))
    }

    /// Take one piece of the stream (one USB transfer, in one or two
    /// slices of the ring) that came off the bus `at`, after `lost`
    /// samples per channel that the host knows it dropped.
    fn piece(&mut self, data: [&[u8]; 2], lost: u64, at: Instant, conn: &Connection) {
        let raw = self.cfg.rate.raw;
        let len = data[0].len() + data[1].len();
        let n = (len / self.cfg.channels.clamp(1, 2) as usize) as u64;
        if n == 0 && lost == 0 {
            // The samples thrown away at the start of a stream: no data,
            // so nothing to check (a restart stays pending).
            return;
        }
        let now = Instant::now();
        if self.binner.is_none() {
            self.binner = Some(self.make_binner(conn));
        }
        let binner = self.binner.as_mut().expect("binner is set");
        match &mut self.check {
            None => {
                let start = writer::start_time(SystemTime::now(), n as usize, raw);
                let header = self.header(conn, start);
                self.writer.send(Msg::Begin(header));
                let binner = self.binner.as_mut().expect("binner is set");
                let mut check = GapCheck::new(
                    raw,
                    Scope6022::<Box<dyn Transport>>::transfer_secs(&self.cfg),
                );
                check.piece(at, n, 0, false);
                self.check = Some(check);
                self.restart = false;
                binner.push(data[0]);
                binner.push(data[1]);
            }
            Some(check) => {
                if let Some(lost) = check.piece(at, n, lost, self.restart) {
                    binner.gap(lost);
                }
                self.restart = false;
                binner.push(data[0]);
                binner.push(data[1]);
            }
        }
        let binner = self.binner.as_mut().expect("binner is set");
        for m in binner.take() {
            self.writer.send(m);
        }
        if binner.take_clipped() {
            self.clip_at = Some(now);
        }
        let recent = self
            .clip_at
            .is_some_and(|t| now.duration_since(t) < Duration::from_secs(1));
        self.req.shared.lock().unwrap().clipped = recent;
    }

    fn finish(mut self) {
        if let Some(b) = &mut self.binner {
            b.finish();
            for m in b.take() {
                self.writer.send(m);
            }
        }
        self.writer.finish();
    }
}

/// The rate the logger streams at for a step (0 = raw), the logged
/// channels and the max ADC rate (for the UI).
pub fn log_rate(step_ns: u64, channels: [bool; 2], max_adc: f64) -> Rate {
    let hw = crate::logger::hw_channels(channels, max_adc);
    crate::logger::adc_for_step(step_ns, hw, max_adc).0
}

/// One block of samples, in volts at the BNC.
#[derive(Clone, Debug)]
pub struct Block {
    pub gen: u64,
    pub cfg: HwConfig,
    /// CH1 and CH2: the mean of the raw samples behind each sample. A
    /// channel that was not sampled is empty.
    pub data: [Vec<f64>; 2],
    /// Lowest and highest raw sample behind each sample (the peak
    /// display). Empty when each sample is one raw sample (then they are
    /// `data`).
    pub min: [Vec<f64>; 2],
    pub max: [Vec<f64>; 2],
    pub clipped: [bool; 2],
    /// Volts at the BNC of the lowest and highest ADC value per channel.
    pub range: [(f64, f64); 2],
    /// Output samples per second.
    pub rate: f64,
    /// A piece of a roll mode stream: append it, do not trigger.
    pub roll: bool,
    /// Where the trigger is, as a fractional sample index, when the block
    /// is a capture around a trigger the stream processing found.
    /// `None`: look for it in the block.
    pub trigger: Option<f64>,
}

/// Why the stream started again by itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Restart {
    /// The processing fell so far behind that the ring was full: data
    /// was dropped.
    Overrun,
    /// The scope or USB lost samples (the reason).
    UsbLoss(String),
}

impl Restart {
    pub fn message(&self) -> String {
        match self {
            Restart::Overrun => {
                "Processing fell behind and samples were lost: the stream started again".into()
            }
            Restart::UsbLoss(why) => format!("USB lost samples ({why}): the stream started again"),
        }
    }
}

pub enum Event {
    /// Progress text, like "Loading firmware".
    Status(String),
    Connected {
        name: String,
        notes: Vec<String>,
    },
    /// The scope went away. The thread keeps trying to open it again.
    Disconnected(String),
    Error(String),
    /// The stream started again after a loss of data.
    Restart(Restart),
}

/// The newest frame the processing made, for the UI. A new one replaces
/// one the UI did not take yet.
#[derive(Default)]
pub struct Latest {
    slot: Mutex<Option<Box<Published>>>,
    fresh: AtomicBool,
}

impl Latest {
    pub fn put(&self, p: Box<Published>) {
        *self.slot.lock().unwrap_or_else(|e| e.into_inner()) = Some(p);
        self.fresh.store(true, Ordering::Release);
    }

    /// The newest frame, if one came since the last call.
    pub fn take(&self) -> Option<Box<Published>> {
        if !self.fresh.swap(false, Ordering::AcqRel) {
            return None;
        }
        self.slot.lock().unwrap_or_else(|e| e.into_inner()).take()
    }

    /// True when a frame is waiting.
    pub fn fresh(&self) -> bool {
        self.fresh.load(Ordering::Acquire)
    }
}

/// Opens the scope. Gets a callback for progress texts.
pub type Opener = Box<dyn FnMut(&mut dyn FnMut(&str)) -> Result<Connection> + Send>;

pub struct Acquisition {
    tx: Sender<Command>,
    rx: Option<Receiver<Event>>,
    latest: Arc<Latest>,
    handle: Option<JoinHandle<()>>,
}

impl Acquisition {
    pub fn spawn(opener: Opener) -> Self {
        Self::spawn_with(opener, STREAM_RING)
    }

    /// [`Acquisition::spawn`] with a stream ring of `ring` bytes (a power
    /// of two; the tests use small ones).
    pub fn spawn_with(opener: Opener, ring: usize) -> Self {
        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, rx) = mpsc::channel();
        let latest = Arc::new(Latest::default());
        let l2 = latest.clone();
        let handle = std::thread::Builder::new()
            .name("acquisition".into())
            .spawn(move || Worker::new(opener, cmd_rx, ev_tx, l2, ring).run())
            .expect("start the acquisition thread");
        Self {
            tx,
            rx: Some(rx),
            latest,
            handle: Some(handle),
        }
    }

    pub fn send(&self, cmd: Command) {
        let _ = self.tx.send(cmd);
    }

    pub fn try_recv(&self) -> Option<Event> {
        self.rx.as_ref()?.try_recv().ok()
    }

    /// Wait up to `timeout` for the next event.
    pub fn recv_timeout(&self, timeout: Duration) -> Option<Event> {
        self.rx.as_ref()?.recv_timeout(timeout).ok()
    }

    /// The newest frame, if one came since the last call.
    pub fn latest(&self) -> Option<Box<Published>> {
        self.latest.take()
    }
}

impl Drop for Acquisition {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        self.rx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

/// The running scope stream and its processing.
struct Live {
    gen: u64,
    cfg: HwConfig,
    ring: Arc<Ring>,
    pipeline: Pipeline,
    /// Checks the transfer times against the data: a scope FIFO that
    /// overflowed loses data silently.
    check: GapCheck,
    /// Bytes at the end of the last transfer mark.
    mark_end: u64,
    /// When data last came.
    data_at: Instant,
    written: u64,
}

struct Worker {
    opener: Opener,
    cmd_rx: Receiver<Command>,
    ev_tx: Sender<Event>,
    latest: Arc<Latest>,
    conn: Option<Connection>,
    want: Option<(u64, HwConfig)>,
    process: Option<(u64, ProcSettings)>,
    running: bool,
    quit: bool,
    last_error: Option<String>,
    log: Option<LogSession>,
    /// The app's calibration, put into each connection.
    app_cal: AppCal,
    /// The stream ring, made at the first stream and used for all.
    ring: Option<Arc<Ring>>,
    ring_size: usize,
    live: Option<Live>,
    /// The stream of the running log.
    log_live: Option<LogLive>,
}

/// The stream of a log: the logger reads the ring as a second reader
/// (the scope's processing does not run meanwhile).
struct LogLive {
    cfg: HwConfig,
    ring: Arc<Ring>,
    /// The next byte to log (the unstable start is skipped).
    cursor: u64,
    /// Bytes at the end of the last transfer mark.
    mark_end: u64,
    /// When data last came, and the bytes then.
    data_at: Instant,
    written: u64,
}

impl LogLive {
    /// Give the log every transfer that came since the last call, each
    /// with its bus time.
    fn take(&mut self, log: &mut LogSession, conn: &Connection) {
        for m in self.ring.take_marks() {
            let from = self.mark_end.max(self.cursor);
            self.mark_end = m.end;
            if m.end > from {
                self.ring
                    .read_bytes(from..m.end, |a, b| log.piece([a, b], 0, m.at, conn));
                self.cursor = m.end;
            }
        }
        let ch = self.cfg.channels.clamp(1, 2) as u64;
        self.ring.keep_from(self.cursor / ch);
    }
}

impl Worker {
    fn new(
        opener: Opener,
        cmd_rx: Receiver<Command>,
        ev_tx: Sender<Event>,
        latest: Arc<Latest>,
        ring_size: usize,
    ) -> Self {
        Self {
            opener,
            cmd_rx,
            ev_tx,
            latest,
            conn: None,
            want: None,
            process: None,
            running: true,
            quit: false,
            last_error: None,
            log: None,
            app_cal: AppCal::default(),
            ring: None,
            ring_size,
            live: None,
            log_live: None,
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Configure { gen, cfg } => self.want = Some((gen, cfg)),
            Command::Process { pgen, p } => {
                self.app_cal = p.cal;
                if let Some(c) = &mut self.conn {
                    c.driver.cal.app = p.cal;
                }
                if let Some(l) = &mut self.live {
                    let newest = l.ring.frames();
                    l.pipeline.set_proc(pgen, (*p).clone(), newest);
                }
                self.process = Some((pgen, *p));
            }
            Command::Run(run) => {
                self.running = run;
                // A log keeps the scope running.
                if !run && self.log.is_none() {
                    self.stop_stream();
                    if let Some(c) = &mut self.conn {
                        let _ = c.driver.stop();
                    }
                }
            }
            Command::StartLog(req) => {
                self.stop_log();
                self.stop_stream();
                self.log = Some(LogSession::new(req));
            }
            Command::StopLog => self.stop_log(),
            Command::Quit => self.quit = true,
        }
    }

    fn stop_log(&mut self) {
        if let Some(mut l) = self.log.take() {
            // Stop the scope, then take the rest of the stream: every
            // sample the scope sent is logged.
            if let Some(c) = &mut self.conn {
                let _ = c.driver.stop();
                if let Some(live) = &mut self.log_live {
                    live.take(&mut l, c);
                }
            }
            self.log_live = None;
            l.finish();
        }
    }

    /// End the scope stream (the scope stops sampling).
    fn stop_stream(&mut self) {
        if self.live.take().is_some() {
            if let Some(c) = &mut self.conn {
                let _ = c.driver.stop();
            }
        }
    }

    /// Handle all waiting commands. With a timeout, wait that long for the
    /// first one.
    fn poll(&mut self, wait: Option<Duration>) {
        if let Some(t) = wait {
            match self.cmd_rx.recv_timeout(t) {
                Ok(cmd) => self.handle(cmd),
                Err(RecvTimeoutError::Timeout) => return,
                Err(RecvTimeoutError::Disconnected) => {
                    self.quit = true;
                    return;
                }
            }
        }
        loop {
            match self.cmd_rx.try_recv() {
                Ok(cmd) => self.handle(cmd),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.quit = true;
                    break;
                }
            }
        }
    }

    /// Send an event. False when the UI is gone.
    fn emit(&self, ev: Event) -> bool {
        self.ev_tx.send(ev).is_ok()
    }

    fn error_once(&mut self, msg: String) -> bool {
        if self.last_error.as_deref() == Some(msg.as_str()) {
            return true;
        }
        self.last_error = Some(msg.clone());
        self.emit(Event::Error(msg))
    }

    /// A stream error: the scope is gone, or try again.
    fn stream_error(&mut self, e: anyhow::Error) {
        self.stop_stream();
        if is_gone(&e) {
            self.conn = None;
            if !self.emit(Event::Disconnected(format!("{e:#}"))) {
                self.quit = true;
            }
            self.poll(Some(Duration::from_millis(500)));
        } else if is_timeout(&e) {
            if !self.error_once("USB timeout, trying again".into()) {
                self.quit = true;
            }
        } else {
            if !self.error_once(format!("{e:#}")) {
                self.quit = true;
            }
            self.poll(Some(Duration::from_millis(200)));
        }
    }

    fn run(mut self) {
        while !self.quit {
            self.poll(None);
            if self.quit {
                break;
            }
            if self.conn.is_none() {
                self.live = None;
                self.log_live = None;
                if !self.connect() {
                    continue;
                }
            }
            if self.log.as_ref().is_some_and(|l| l.writer_done()) {
                // The size limit (or a write error) ended the log.
                self.stop_log();
                continue;
            }
            if let Some(cfg) = self.log.as_ref().map(|l| l.cfg) {
                self.log_step(cfg);
                continue;
            }
            let (Some((gen, cfg)), Some((pgen, p)), true) =
                (self.want, self.process.clone(), self.running)
            else {
                self.stop_stream();
                self.poll(Some(Duration::from_millis(50)));
                continue;
            };
            if self
                .live
                .as_ref()
                .is_none_or(|l| l.gen != gen || l.cfg != cfg)
            {
                self.stop_stream();
                if let Err(e) = self.start_stream(gen, cfg, pgen, p) {
                    self.stream_error(e);
                    continue;
                }
            }
            self.process_stream();
        }
        self.stop_log();
        self.stop_stream();
        if let Some(c) = &mut self.conn {
            let _ = c.driver.stop();
        }
    }

    /// Start the scope stream and its processing.
    fn start_stream(&mut self, gen: u64, cfg: HwConfig, pgen: u64, p: ProcSettings) -> Result<()> {
        let conn = self.conn.as_mut().expect("connected");
        let ring = self
            .ring
            .get_or_insert_with(|| Arc::new(Ring::new(self.ring_size)))
            .clone();
        conn.driver.set_ring(ring);
        let ring = conn.driver.start_stream(&cfg)?;
        let pipeline = Pipeline::new(gen, pgen, cfg, &conn.driver.cal, p, ring.frames());
        self.live = Some(Live {
            gen,
            cfg,
            ring,
            pipeline,
            check: GapCheck::new(
                cfg.rate.raw,
                Scope6022::<Box<dyn Transport>>::transfer_secs(&cfg),
            ),
            mark_end: 0,
            data_at: Instant::now(),
            written: 0,
        });
        Ok(())
    }

    /// Process what the stream brought; wait a little for more.
    fn process_stream(&mut self) {
        let live = self.live.as_mut().expect("streaming");
        let ring = live.ring.clone();
        if let Some(Failure::Usb(e)) = ring.failure() {
            let e = anyhow::Error::new(e).context("reading samples failed");
            if is_gone(&e) {
                self.stream_error(e);
            } else {
                // A transfer that failed: its data is lost.
                self.restart(Restart::UsbLoss(format!("{e:#}")));
            }
            return;
        }
        if ring.overrun() > 0 {
            self.restart(Restart::Overrun);
            return;
        }
        let ch = live.cfg.channels.clamp(1, 2) as u64;
        let mut loss = None;
        for m in ring.take_marks() {
            let n = (m.end - live.mark_end) / ch;
            live.mark_end = m.end;
            if m.flags & MARK_SHORT != 0 {
                loss = Some("a short transfer".to_string());
            } else if let Some(lost) = live.check.piece(m.at, n, 0, false) {
                loss = Some(format!(
                    "{:.1} ms missing",
                    lost as f64 / live.cfg.rate.raw * 1e3
                ));
            }
        }
        if let Some(why) = loss {
            self.restart(Restart::UsbLoss(why));
            return;
        }
        let latest = self.latest.clone();
        live.pipeline.step(&ring, &mut |p| latest.put(p));
        self.last_error = None;
        if live.pipeline.single_done() {
            // The single trigger fired: the scope stops.
            self.running = false;
            self.stop_stream();
            return;
        }
        let written = ring.written();
        if written > live.written {
            live.written = written;
            live.data_at = Instant::now();
        }
        let silent = Duration::from_secs_f64(
            Scope6022::<Box<dyn Transport>>::transfer_secs(&live.cfg) * 2.0 + 1.0,
        );
        if live.data_at.elapsed() > silent {
            let e = anyhow::Error::new(rusb::Error::Timeout).context(format!(
                "no samples from the scope for {:.1} s",
                silent.as_secs_f64()
            ));
            self.stream_error(e);
            return;
        }
        // All of it is done: wait for more (commands are looked at often).
        ring.wait(ring.frames(), Duration::from_millis(10));
    }

    /// Start the stream again after a loss, and tell the UI.
    fn restart(&mut self, why: Restart) {
        self.stop_stream();
        if !self.emit(Event::Restart(why)) {
            self.quit = true;
        }
    }

    /// Log what the stream brought; start the log's stream when it does
    /// not run. A loss or an error marks a gap in the log (the clock
    /// tells how long) and starts the stream again.
    fn log_step(&mut self, cfg: HwConfig) {
        if self.log_live.as_ref().is_none_or(|l| l.cfg != cfg) {
            self.log_live = None;
            let conn = self.conn.as_mut().expect("connected");
            let ring = self
                .ring
                .get_or_insert_with(|| Arc::new(Ring::new(self.ring_size)))
                .clone();
            conn.driver.set_ring(ring);
            match conn.driver.start_stream(&cfg) {
                Ok(ring) => {
                    let skip = (SKIP_SAMPLES * cfg.channels.clamp(1, 2) as usize) as u64;
                    self.log_live = Some(LogLive {
                        cfg,
                        ring,
                        cursor: skip,
                        mark_end: 0,
                        data_at: Instant::now(),
                        written: 0,
                    });
                }
                Err(e) => {
                    self.log_error(e);
                    return;
                }
            }
        }
        let (Some(live), Some(log), Some(conn)) = (&mut self.log_live, &mut self.log, &self.conn)
        else {
            return;
        };
        let ring = live.ring.clone();
        let failed = ring.failure();
        // What came before a loss is whole: log it first.
        live.take(log, conn);
        self.last_error = None;
        if let Some(Failure::Usb(e)) = failed {
            self.log_error(anyhow::Error::new(e).context("reading samples failed"));
            return;
        }
        if ring.overrun() > 0 {
            // The PC could not keep up: a gap, and the stream again.
            log.restart = true;
            self.log_live = None;
            if let Some(c) = &mut self.conn {
                let _ = c.driver.stop();
            }
            return;
        }
        let written = ring.written();
        if written > live.written {
            live.written = written;
            live.data_at = Instant::now();
        }
        let silent = Duration::from_secs_f64(
            Scope6022::<Box<dyn Transport>>::transfer_secs(&cfg) * 2.0 + 1.0,
        );
        if live.data_at.elapsed() > silent {
            self.log_error(anyhow::Error::new(rusb::Error::Timeout).context(format!(
                "no samples from the scope for {:.1} s",
                silent.as_secs_f64()
            )));
            return;
        }
        ring.wait(ring.frames(), Duration::from_millis(10));
    }

    /// The log's stream failed: a gap in the log, the stream again.
    fn log_error(&mut self, e: anyhow::Error) {
        if let Some(l) = &mut self.log {
            l.restart = true;
        }
        self.log_live = None;
        if let Some(c) = &mut self.conn {
            let _ = c.driver.stop();
        }
        if is_gone(&e) {
            self.conn = None;
            if !self.emit(Event::Disconnected(format!("{e:#}"))) {
                self.quit = true;
            }
            self.poll(Some(Duration::from_millis(500)));
        } else if is_timeout(&e) {
            if !self.error_once("USB timeout, trying again".into()) {
                self.quit = true;
            }
        } else {
            if !self.error_once(format!("{e:#}")) {
                self.quit = true;
            }
            self.poll(Some(Duration::from_millis(200)));
        }
    }

    /// Try to open the scope. On failure, wait a bit (still handling
    /// commands) and return false.
    fn connect(&mut self) -> bool {
        let tx = self.ev_tx.clone();
        let mut status = |s: &str| {
            let _ = tx.send(Event::Status(s.to_string()));
        };
        match (self.opener)(&mut status) {
            Ok(mut conn) => {
                conn.driver.cal.app = self.app_cal;
                let ev = Event::Connected {
                    name: conn.name.clone(),
                    notes: conn.notes.clone(),
                };
                self.conn = Some(conn);
                self.last_error = None;
                if !self.emit(ev) {
                    self.quit = true;
                }
                true
            }
            Err(e) => {
                if !self.error_once(format!("{e:#}")) {
                    self.quit = true;
                }
                self.poll(Some(Duration::from_secs(1)));
                false
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;
    use crate::device::fake::{FakeScope, Wave};
    use crate::device::hantek::Scope6022;
    use crate::device::protocol::{choose_rate, MAX_ADC_DEFAULT};
    use crate::device::Driver;
    use crate::scope::TrigState;
    use crate::settings::{Settings, TriggerMode};
    use crate::stream::pipeline::capture_len;

    fn fake_opener(fake: FakeScope) -> Opener {
        Box::new(move |_status| {
            let mut driver: Driver = Scope6022::new(Box::new(fake.clone()), "T");
            let notes = driver.load_calibration(None);
            Ok(Connection {
                driver,
                name: "fake".into(),
                notes,
            })
        })
    }

    fn settings(time_div: f64) -> Settings {
        let mut s = Settings::default();
        s.time_idx = crate::scope::time_index(time_div);
        s.trigger.level = 1.0;
        s
    }

    fn cfg_of(s: &Settings) -> HwConfig {
        HwConfig {
            rate: choose_rate(s.coarse_screen_time(), 2, MAX_ADC_DEFAULT),
            channels: 2,
            gains: [5, 5],
            dc: [true; 2],
            cal_freq: 1000,
            roll: s.roll(),
        }
    }

    /// Settings for the hardware and the processing, both generation `gen`.
    fn go(acq: &Acquisition, gen: u64, s: &Settings) {
        acq.send(Command::Configure {
            gen,
            cfg: cfg_of(s),
        });
        acq.send(Command::Process {
            pgen: gen,
            p: Box::new(ProcSettings::of(s, gen)),
        });
    }

    /// The next published frame of generation `gen`.
    fn next_frame(acq: &Acquisition, gen: u64) -> Box<Published> {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(10) {
            while acq.try_recv().is_some() {}
            if let Some(p) = acq.latest() {
                if p.gen == gen && p.frame.is_some() {
                    return p;
                }
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        panic!("no frame");
    }

    /// Wait for an event that `want` takes.
    fn wait_event(acq: &Acquisition, secs: u64, want: impl Fn(&Event) -> bool) -> bool {
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_secs(secs) {
            if let Some(e) = acq.recv_timeout(Duration::from_millis(20)) {
                if want(&e) {
                    return true;
                }
            }
            acq.latest();
        }
        false
    }

    #[test]
    fn connects_and_streams_frames_with_generation() {
        let fake = FakeScope::new([Wave::CalOut, Wave::Dc(-1.0)]);
        let acq = Acquisition::spawn(fake_opener(fake));
        let s = settings(1e-3);
        go(&acq, 1, &s);
        let p = next_frame(&acq, 1);
        assert_eq!(p.state, TrigState::Triggered);
        let f = p.frame.as_ref().unwrap();
        let (d, _, _) = ProcSettings::of(&s, 1).window(f.rate);
        assert_eq!(f.traces[0].as_ref().unwrap().len(), capture_len(d));
        let roll = settings(0.2);
        go(&acq, 2, &roll);
        let p = next_frame(&acq, 2);
        assert_eq!(p.state, TrigState::Roll);
        assert!(p.frame.as_ref().unwrap().roll);
    }

    #[test]
    fn stop_stops_the_scope() {
        let fake = FakeScope::new([Wave::Dc(0.0); 2]);
        let acq = Acquisition::spawn(fake_opener(fake.clone()));
        go(&acq, 1, &settings(1e-3));
        next_frame(&acq, 1);
        acq.send(Command::Run(false));
        std::thread::sleep(Duration::from_millis(200));
        let before = fake.lock().bulk_bytes;
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(fake.lock().bulk_bytes, before);
        assert!(!fake.lock().sampling);
        acq.latest();
        acq.send(Command::Run(true));
        next_frame(&acq, 1);
    }

    #[test]
    fn reconnects_after_unplug() {
        let fake = FakeScope::new([Wave::Dc(0.0); 2]);
        let opens = Arc::new(Mutex::new(0));
        let f2 = fake.clone();
        let o2 = opens.clone();
        let opener: Opener = Box::new(move |_| {
            *o2.lock().unwrap() += 1;
            if f2.lock().unplugged {
                anyhow::bail!("no DSO-6022BE found");
            }
            let mut driver: Driver = Scope6022::new(Box::new(f2.clone()), "T");
            driver.load_calibration(None);
            Ok(Connection {
                driver,
                name: "fake".into(),
                notes: vec![],
            })
        });
        let acq = Acquisition::spawn(opener);
        go(&acq, 1, &settings(1e-3));
        next_frame(&acq, 1);
        fake.lock().unplugged = true;
        let mut saw_disconnect = false;
        let mut saw_error = false;
        for _ in 0..100 {
            match acq.recv_timeout(Duration::from_millis(50)) {
                Some(Event::Disconnected(_)) => saw_disconnect = true,
                Some(Event::Error(e)) if saw_disconnect => {
                    assert!(e.contains("no DSO-6022BE"));
                    saw_error = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(saw_disconnect && saw_error);
        fake.lock().unplugged = false;
        assert!(wait_event(&acq, 5, |e| matches!(
            e,
            Event::Connected { .. }
        )));
        next_frame(&acq, 1);
        assert!(*opens.lock().unwrap() >= 3);
    }

    #[test]
    fn quit_stops_sampling() {
        let fake = FakeScope::new([Wave::Dc(0.0); 2]);
        let acq = Acquisition::spawn(fake_opener(fake.clone()));
        go(&acq, 1, &settings(1e-3));
        next_frame(&acq, 1);
        drop(acq);
        let calls = fake.lock().calls.clone();
        let last = calls.last().unwrap();
        assert_eq!((last.request, last.data.clone()), (0xE3, vec![0]));
        assert!(fake.lock().stream.is_none());
    }

    #[test]
    fn a_full_ring_restarts_the_stream() {
        // A capture of 10 ms at 32 MB/s does not fit 64 kB: the reader
        // must hold the data back, the ring fills.
        let fake = FakeScope::new([Wave::CalOut, Wave::Dc(0.0)]);
        fake.lock().realtime = true;
        let acq = Acquisition::spawn_with(fake_opener(fake), 1 << 16);
        go(&acq, 1, &settings(1e-3));
        assert!(wait_event(&acq, 10, |e| matches!(
            e,
            Event::Restart(Restart::Overrun)
        )));
    }

    #[test]
    fn lost_samples_and_transfer_errors_restart_the_stream() {
        let fake = FakeScope::new([Wave::CalOut, Wave::Dc(0.0)]);
        let acq = Acquisition::spawn(fake_opener(fake.clone()));
        go(&acq, 1, &settings(1e-3));
        next_frame(&acq, 1);
        // The scope's FIFO lost 100 ms: the bus times say so.
        fake.lock().drop_samples = 1_600_000;
        assert!(wait_event(&acq, 10, |e| matches!(
            e,
            Event::Restart(Restart::UsbLoss(_))
        )));
        next_frame(&acq, 1);
        fake.lock().fail_stream = Some(rusb::Error::Overflow);
        assert!(wait_event(&acq, 10, |e| matches!(
            e,
            Event::Restart(Restart::UsbLoss(w)) if w.contains("Overflow")
        )));
        // It goes on after that.
        next_frame(&acq, 1);
    }

    #[test]
    fn single_stops_the_stream_after_its_trigger() {
        let fake = FakeScope::new([Wave::CalOut, Wave::Dc(0.0)]);
        let acq = Acquisition::spawn(fake_opener(fake.clone()));
        let mut s = settings(1e-3);
        s.trigger.mode = TriggerMode::Single;
        go(&acq, 1, &s);
        let p = next_frame(&acq, 1);
        assert!(p.single_done);
        assert_eq!(p.triggers, 1);
        let t0 = Instant::now();
        while fake.lock().sampling {
            assert!(t0.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn a_slow_ui_gets_the_newest_frame_and_every_trigger() {
        // The cal wave in real time; the UI looks only after 1 s.
        let fake = FakeScope::new([Wave::CalOut, Wave::Dc(0.0)]);
        fake.lock().realtime = true;
        let acq = Acquisition::spawn(fake_opener(fake));
        let s = settings(1e-3);
        go(&acq, 1, &s);
        next_frame(&acq, 1);
        std::thread::sleep(Duration::from_secs(1));
        let p = next_frame(&acq, 1);
        let f = p.frame.as_ref().unwrap();
        let dt = 1.0 / f.rate;
        // 1 kHz edges, 10 ms screen with the trigger in the middle: each
        // trigger rearms 5 ms (and the margin) after its edge, so the
        // next is 6 ms later.
        let secs = p.points as f64 * dt;
        let want = (secs - 0.005 - 2048.0 / 16e6) / 0.006;
        assert!(
            (p.triggers as f64 - want).abs() < 2.5,
            "{} triggers in {secs:.3} s, want {want:.1}",
            p.triggers
        );
        // The frame is of the newest trigger: the capture after it is
        // still being filled, and the publish is at most a frame time old.
        let t = p.trigger_at.unwrap() * dt;
        assert!(secs - t < 0.006 + 0.005 + 0.02, "{secs} {t}");
        assert!(p.triggers > 150);
    }
}

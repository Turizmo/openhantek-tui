//! Acquisition thread: talks to the scope and sends sample blocks to the UI.
//! The UI sends settings; each settings change gets a new generation
//! number, so the UI can drop blocks made with old settings.

use std::sync::mpsc::{
    self, Receiver, RecvTimeoutError, Sender, SyncSender, TryRecvError, TrySendError,
};
use std::thread::JoinHandle;
use std::time::{Duration, Instant, SystemTime};

use anyhow::Result;

use crate::device::hantek::{RawPiece, Scope6022};
use crate::device::protocol::{AppCal, HwConfig, Rate};
use crate::device::transport::{is_gone, is_timeout, Transport};
use crate::device::Connection;
use crate::logger::binner::Binner;
use crate::logger::format::{ChannelInfo, Header};
use crate::logger::gapcheck::GapCheck;
use crate::logger::writer::{self, LogState, Msg, Shared, Writer};

pub enum Command {
    /// Use these hardware settings from now on.
    Configure {
        gen: u64,
        cfg: HwConfig,
    },
    /// Capture (true) or stop (false).
    Run(bool),
    /// Start the envelope logger. While it runs, the scope streams with
    /// the logger's settings, `Configure` waits until the log stops, and
    /// no blocks go to the UI.
    StartLog(LogRequest),
    StopLog,
    /// Use the app's own calibration (from the config file) from now on,
    /// also after a reconnect. Never written to the scope.
    Calibration(AppCal),
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

    /// Take one piece of the stream.
    fn piece(&mut self, cap: &RawPiece, conn: &Connection) {
        let raw = self.cfg.rate.raw;
        let n = (cap.data.len() / self.cfg.channels.clamp(1, 2) as usize) as u64;
        if n == 0 && cap.lost == 0 {
            // The samples thrown away at the start of a stream: no data,
            // so nothing to check (a restart stays pending).
            return;
        }
        let now = Instant::now();
        let at = cap.at;
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
                binner.push(&cap.data);
            }
            Some(check) => {
                if let Some(lost) = check.piece(at, n, cap.lost, self.restart) {
                    binner.gap(lost);
                }
                self.restart = false;
                binner.push(&cap.data);
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
    /// Boxed: a block is big, the other events are small.
    Block(Box<Block>),
}

/// Opens the scope. Gets a callback for progress texts.
pub type Opener = Box<dyn FnMut(&mut dyn FnMut(&str)) -> Result<Connection> + Send>;

pub struct Acquisition {
    tx: Sender<Command>,
    rx: Option<Receiver<Event>>,
    handle: Option<JoinHandle<()>>,
}

impl Acquisition {
    pub fn spawn(opener: Opener) -> Self {
        let (tx, cmd_rx) = mpsc::channel();
        let (ev_tx, rx) = mpsc::sync_channel(8);
        let handle = std::thread::Builder::new()
            .name("acquisition".into())
            .spawn(move || Worker::new(opener, cmd_rx, ev_tx).run())
            .expect("start the acquisition thread");
        Self {
            tx,
            rx: Some(rx),
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
}

impl Drop for Acquisition {
    fn drop(&mut self) {
        let _ = self.tx.send(Command::Quit);
        // Drop our end, so a blocked send in the thread returns.
        self.rx = None;
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

struct Worker {
    opener: Opener,
    cmd_rx: Receiver<Command>,
    ev_tx: SyncSender<Event>,
    conn: Option<Connection>,
    want: Option<(u64, HwConfig)>,
    running: bool,
    quit: bool,
    last_error: Option<String>,
    log: Option<LogSession>,
    /// The app's calibration, put into each connection.
    app_cal: AppCal,
}

impl Worker {
    fn new(opener: Opener, cmd_rx: Receiver<Command>, ev_tx: SyncSender<Event>) -> Self {
        Self {
            opener,
            cmd_rx,
            ev_tx,
            conn: None,
            want: None,
            running: true,
            quit: false,
            last_error: None,
            log: None,
            app_cal: AppCal::default(),
        }
    }

    fn handle(&mut self, cmd: Command) {
        match cmd {
            Command::Configure { gen, cfg } => self.want = Some((gen, cfg)),
            Command::Run(run) => {
                self.running = run;
                // A log keeps the scope running.
                if !run && self.log.is_none() {
                    if let Some(c) = &mut self.conn {
                        let _ = c.driver.stop();
                    }
                }
            }
            Command::StartLog(req) => {
                self.stop_log();
                self.log = Some(LogSession::new(req));
            }
            Command::StopLog => self.stop_log(),
            Command::Calibration(cal) => {
                self.app_cal = cal;
                if let Some(c) = &mut self.conn {
                    c.driver.cal.app = cal;
                }
            }
            Command::Quit => self.quit = true,
        }
    }

    fn stop_log(&mut self) {
        if let Some(mut l) = self.log.take() {
            // The rest of the stream first: every sample the scope sent.
            if let Some(c) = &mut self.conn {
                if let Ok(Some(p)) = c.driver.stop_raw() {
                    if !p.data.is_empty() {
                        l.piece(&p, c);
                    }
                }
            }
            l.finish();
            // Start the normal capture from scratch.
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

    fn run(mut self) {
        while !self.quit {
            self.poll(None);
            if self.quit {
                break;
            }
            if self.conn.is_none() && !self.connect() {
                continue;
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
            let (Some((gen, cfg)), true) = (self.want, self.running) else {
                self.poll(Some(Duration::from_millis(50)));
                continue;
            };
            let conn = self.conn.as_mut().expect("connected");
            let result = if cfg.roll {
                conn.driver.stream(&cfg)
            } else {
                conn.driver.capture(&cfg)
            };
            match result {
                Ok(cap) => {
                    self.last_error = None;
                    let block = Block {
                        gen,
                        cfg,
                        data: cap.data,
                        min: cap.min,
                        max: cap.max,
                        clipped: cap.clipped,
                        range: cap.range,
                        rate: cap.rate,
                        roll: cfg.roll,
                        trigger: None,
                    };
                    if cfg.roll {
                        // Roll pieces must all arrive, or the trace gets gaps.
                        if !self.emit(Event::Block(Box::new(block))) {
                            break;
                        }
                    } else if let Err(TrySendError::Disconnected(_)) =
                        self.ev_tx.try_send(Event::Block(Box::new(block)))
                    {
                        // A full queue just drops the block: the UI is behind.
                        break;
                    }
                }
                Err(e) if is_gone(&e) => {
                    self.conn = None;
                    if !self.emit(Event::Disconnected(format!("{e:#}"))) {
                        break;
                    }
                    self.poll(Some(Duration::from_millis(500)));
                }
                Err(e) if is_timeout(&e) => {
                    if !self.error_once("USB timeout, trying again".into()) {
                        break;
                    }
                }
                Err(e) => {
                    if !self.error_once(format!("{e:#}")) {
                        break;
                    }
                    self.poll(Some(Duration::from_millis(200)));
                }
            }
        }
        self.stop_log();
        if let Some(c) = &mut self.conn {
            let _ = c.driver.stop();
        }
    }

    /// Read one piece of the log stream.
    fn log_step(&mut self, cfg: HwConfig) {
        let conn = self.conn.as_mut().expect("connected");
        match conn.driver.stream_raw(&cfg) {
            Ok(cap) => {
                self.last_error = None;
                let conn = self.conn.as_ref().expect("connected");
                if let Some(l) = &mut self.log {
                    l.piece(&cap, conn);
                }
            }
            Err(e) => {
                if let Some(l) = &mut self.log {
                    l.restart = true;
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
mod tests {
    use super::*;
    use crate::device::fake::{FakeScope, Wave};
    use crate::device::hantek::Scope6022;
    use crate::device::protocol::{choose_rate, MAX_ADC_DEFAULT};
    use crate::device::Driver;
    use std::sync::{Arc, Mutex};

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

    fn cfg(roll: bool) -> HwConfig {
        HwConfig {
            rate: choose_rate(if roll { 2.0 } else { 0.01 }, 2, MAX_ADC_DEFAULT),
            channels: 2,
            gains: [5, 5],
            dc: [true; 2],
            cal_freq: 1000,
            roll,
        }
    }

    fn next_block(acq: &Acquisition) -> Block {
        for _ in 0..200 {
            if let Some(Event::Block(b)) = acq.recv_timeout(Duration::from_millis(50)) {
                return *b;
            }
        }
        panic!("no block");
    }

    #[test]
    fn connects_and_streams_blocks_with_generation() {
        let fake = FakeScope::new([Wave::Dc(1.0), Wave::Dc(-1.0)]);
        let acq = Acquisition::spawn(fake_opener(fake));
        acq.send(Command::Configure {
            gen: 1,
            cfg: cfg(false),
        });
        let b = next_block(&acq);
        assert_eq!(b.gen, 1);
        assert_eq!(b.data[0].len(), crate::device::protocol::BLOCK_SAMPLES);
        acq.send(Command::Configure {
            gen: 2,
            cfg: cfg(true),
        });
        // Old blocks may still come; a gen 2 roll block must follow.
        let mut b = next_block(&acq);
        while b.gen != 2 {
            b = next_block(&acq);
        }
        assert!(b.roll);
    }

    #[test]
    fn stop_stops_the_scope() {
        let fake = FakeScope::new([Wave::Dc(0.0); 2]);
        let acq = Acquisition::spawn(fake_opener(fake.clone()));
        acq.send(Command::Configure {
            gen: 1,
            cfg: cfg(false),
        });
        next_block(&acq);
        acq.send(Command::Run(false));
        std::thread::sleep(Duration::from_millis(200));
        while acq.try_recv().is_some() {}
        let before = fake.lock().bulk_bytes;
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(fake.lock().bulk_bytes, before);
        assert!(!fake.lock().sampling);
        acq.send(Command::Run(true));
        next_block(&acq);
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
        acq.send(Command::Configure {
            gen: 1,
            cfg: cfg(false),
        });
        next_block(&acq);
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
        let mut connected = false;
        for _ in 0..100 {
            if let Some(Event::Connected { .. }) = acq.recv_timeout(Duration::from_millis(50)) {
                connected = true;
                break;
            }
        }
        assert!(connected);
        next_block(&acq);
        assert!(*opens.lock().unwrap() >= 3);
    }

    #[test]
    fn quit_stops_sampling() {
        let fake = FakeScope::new([Wave::Dc(0.0); 2]);
        let acq = Acquisition::spawn(fake_opener(fake.clone()));
        acq.send(Command::Configure {
            gen: 1,
            cfg: cfg(false),
        });
        next_block(&acq);
        drop(acq);
        let calls = fake.lock().calls.clone();
        let last = calls.last().unwrap();
        assert_eq!((last.request, last.data.clone()), (0xE3, vec![0]));
    }
}

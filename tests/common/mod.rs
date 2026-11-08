//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use openhantek_tui::acquire::{Acquisition, Block, Opener};
use openhantek_tui::app::App;
use openhantek_tui::device::fake::{FakeScope, Wave};
use openhantek_tui::device::hantek::Scope6022;
use openhantek_tui::device::protocol::BLOCK_SAMPLES;
use openhantek_tui::device::{Connection, Driver};
use openhantek_tui::settings::Settings;

pub const SINE_1K: Wave = Wave::Sine {
    freq: 1000.0,
    amp: 1.5,
    dc: 0.0,
};

pub fn driver(fake: &FakeScope) -> Driver {
    let mut d: Driver = Scope6022::new(Box::new(fake.clone()), "TEST");
    d.load_calibration(None);
    d
}

pub fn opener(fake: FakeScope) -> Opener {
    Box::new(move |_| {
        Ok(Connection {
            driver: driver(&fake),
            name: "fake".into(),
            notes: vec![],
        })
    })
}

/// An app with the acquisition thread on a fake scope.
pub fn live_app(waves: [Wave; 2], settings: Settings) -> (App, FakeScope) {
    let fake = FakeScope::new(waves);
    let acq = Acquisition::spawn(opener(fake.clone()));
    (App::new(settings, Some(acq)), fake)
}

/// Tick the app until `done` is true, or panic after `secs`.
pub fn tick_until(app: &mut App, secs: f64, mut done: impl FnMut(&App) -> bool) {
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        app.tick();
        if done(app) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!("timed out; state {:?}", app.scope.state);
}

/// One block for the app's current hardware settings from the fake,
/// without the thread. Good for UI tests that need exact control. A fresh
/// start of the stream each time (the signal time goes on): in roll mode
/// a piece of about one USB transfer, else [`BLOCK_SAMPLES`] samples.
pub fn block_for(app: &App, d: &mut Driver, fake: &FakeScope) -> Block {
    let cfg = app.scope.hw.expect("hw config");
    if d.streaming().is_some() {
        d.stop().unwrap();
    }
    let os = cfg.rate.oversample;
    let points = if cfg.roll {
        let ch = cfg.channels.clamp(1, 2) as usize;
        (Driver::stream_setup(&cfg).transfer / ch / os).max(1)
    } else {
        BLOCK_SAMPLES
    };
    let frames = (2048 / os + 1) * os + points * os;
    let raw = fake.raw(&cfg, frames);
    d.raw_block(&cfg, &raw, app.scope.gen)
}

//! Helpers shared by the integration tests.
#![allow(dead_code)]

use std::time::{Duration, Instant};

use openhantek_tui::acquire::{Acquisition, Block, Opener};
use openhantek_tui::app::App;
use openhantek_tui::device::fake::{FakeScope, Wave};
use openhantek_tui::device::hantek::Scope6022;
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

/// Capture one block for the app's current hardware settings, without the
/// thread. Good for UI tests that need exact control.
pub fn block_for(app: &App, d: &mut Driver) -> Block {
    let cfg = app.scope.hw.expect("hw config");
    let cap = if cfg.roll {
        d.stream(&cfg).unwrap()
    } else {
        d.capture(&cfg).unwrap()
    };
    Block {
        gen: app.scope.gen,
        cfg,
        data: cap.data,
        clipped: cap.clipped,
        rate: cap.rate,
        roll: cfg.roll,
    }
}

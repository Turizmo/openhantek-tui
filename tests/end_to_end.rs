//! The whole chain on the fake scope: acquisition thread, driver, scope
//! logic and app. Checks that what we measure matches what the fake makes.
#![allow(clippy::field_reassign_with_default)]

mod common;

use common::*;
use openhantek_tui::device::fake::Wave;
use openhantek_tui::scope::{time_index, TrigState};
use openhantek_tui::settings::{Settings, TriggerMode};

fn close(a: f64, b: f64, rel: f64) -> bool {
    (a - b).abs() <= rel * b.abs()
}

#[test]
fn measures_the_generated_signals() {
    let square = Wave::Square {
        freq: 2500.0,
        low: 0.0,
        high: 3.3,
        duty: 0.3,
    };
    let (mut app, _fake) = live_app([SINE_1K, square], Settings::default());
    app.scope.settings.trigger.level = 0.2;
    tick_until(&mut app, 10.0, |a| a.scope.state == TrigState::Triggered);
    // Let a few frames pass, so the measure uses settled data.
    for _ in 0..3 {
        let gen = app.scope.frame.as_ref().map(|f| f.t0);
        tick_until(&mut app, 5.0, |a| {
            a.scope.frame.as_ref().map(|f| f.t0) != gen
        });
    }
    let m1 = app.scope.measure(0).unwrap();
    assert!(close(m1.freq.unwrap(), 1000.0, 0.002), "{:?}", m1.freq);
    assert!(close(m1.pk_pk, 3.0, 0.05), "{}", m1.pk_pk);
    assert!(m1.mean.abs() < 0.05);
    let m2 = app.scope.measure(1).unwrap();
    assert!(close(m2.freq.unwrap(), 2500.0, 0.002), "{:?}", m2.freq);
    assert!(close(m2.duty.unwrap(), 0.3, 0.02), "{:?}", m2.duty);
    assert!(close(m2.top, 3.3, 0.02), "{}", m2.top);
}

#[test]
fn trigger_edge_sits_at_the_trigger_point() {
    let (mut app, _fake) = live_app([SINE_1K, Wave::Dc(0.0)], Settings::default());
    app.scope.settings.trigger.level = 0.5;
    tick_until(&mut app, 10.0, |a| a.scope.state == TrigState::Triggered);
    let f = app.scope.frame.as_ref().unwrap();
    let tr = f.traces[0].as_ref().unwrap();
    let i = (-f.t0 / f.dt).floor() as usize;
    // Rising through 0.5 V at the trigger point (8-bit steps: 40 mV).
    assert!(
        tr[i] <= 0.5 + 0.05 && tr[i + 1] >= 0.5 - 0.05,
        "{} {}",
        tr[i],
        tr[i + 1]
    );
    assert!(tr[i + 20] > tr[i]);
}

#[test]
fn single_mode_stops_after_a_trigger() {
    let (mut app, fake) = live_app([SINE_1K, Wave::Dc(0.0)], Settings::default());
    app.scope.settings.trigger.mode = TriggerMode::Single;
    app.scope.arm_single();
    tick_until(&mut app, 10.0, |a| a.scope.state == TrigState::Stopped);
    assert!(!app.scope.running);
    assert!(app.scope.frame.as_ref().unwrap().triggered);
    // The app tells the thread to stop; the fake stops sampling.
    tick_until(&mut app, 5.0, |_| !fake.lock().sampling);
}

#[test]
fn ch1_only_uses_one_channel_and_a_faster_rate() {
    let mut s = Settings::default();
    s.channels[1].enabled = false;
    s.time_idx = time_index(20e-6);
    let (mut app, fake) = live_app([SINE_1K, Wave::Dc(0.0)], s);
    tick_until(&mut app, 10.0, |a| a.scope.frame.is_some());
    assert_eq!(fake.lock().channels, 1);
    let f = app.scope.frame.as_ref().unwrap();
    // The top rate of 1-channel mode, not decimated.
    assert_eq!(f.rate, 30e6);
    assert_eq!(fake.lock().rate_id, 30);
    assert!(f.traces[1].is_none());
}

#[test]
fn roll_mode_scrolls_with_new_data() {
    let mut s = Settings::default();
    s.time_idx = time_index(200e-3);
    let slow = Wave::Sine {
        freq: 2.0,
        amp: 1.0,
        dc: 0.0,
    };
    let (mut app, _fake) = live_app([slow, Wave::Dc(0.0)], s);
    tick_until(&mut app, 10.0, |a| a.scope.state == TrigState::Roll);
    let n0 = app.scope.frame.as_ref().unwrap().traces[0]
        .as_ref()
        .unwrap()
        .len();
    tick_until(&mut app, 10.0, |a| {
        a.scope.frame.as_ref().unwrap().traces[0]
            .as_ref()
            .unwrap()
            .len()
            > n0 + 100
    });
}

#[test]
fn time_base_change_drops_old_blocks() {
    let (mut app, _fake) = live_app([SINE_1K, Wave::Dc(0.0)], Settings::default());
    tick_until(&mut app, 10.0, |a| a.scope.frame.is_some());
    let gen = app.scope.gen;
    app.handle_key(key('→'));
    assert_eq!(app.scope.gen, gen + 1);
    // The next frame uses the new rate (2 ms/div: 500 kS/s).
    tick_until(&mut app, 10.0, |a| {
        a.scope.frame.as_ref().unwrap().rate == 500e3
    });
}

#[test]
fn csv_export_has_the_screen() {
    let (mut app, _fake) = live_app([SINE_1K, Wave::CalOut], Settings::default());
    tick_until(&mut app, 10.0, |a| a.scope.frame.is_some());
    let dir = std::env::temp_dir().join(format!("oht-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = app.export_to(&dir, false).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines = text.lines();
    assert_eq!(lines.next().unwrap(), "time_s,CH1_V,CH2_V");
    let rows: Vec<Vec<f64>> = lines
        .map(|l| l.split(',').map(|v| v.parse().unwrap()).collect())
        .collect();
    // One screen at 1 MS/s, 1 ms/div: about 10001 rows from -5 ms to +5 ms.
    assert!((rows.len() as i64 - 10_001).abs() <= 1, "{}", rows.len());
    assert!((rows[0][0] + 5e-3).abs() < 2e-6);
    assert!(rows.iter().all(|r| r[2] > -0.2 && r[2] < 2.2));
    let json = app.export_to(&dir, true).unwrap();
    let doc: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(json).unwrap()).unwrap();
    assert_eq!(doc["traces"]["CH1_V"].as_array().unwrap().len(), rows.len());
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn autoset_on_the_fake_scope() {
    let (mut app, _fake) = live_app([SINE_1K, Wave::CalOut], Settings::default());
    app.handle_key(key('a'));
    tick_until(&mut app, 15.0, |a| {
        !a.scope.autoset_running() && a.scope.state == TrigState::Triggered
    });
    let s = &app.scope.settings;
    // 3 Vpp sine in half the screen: 1 V/div; 1 kHz: 500 us/div.
    assert_eq!(s.channels[0].volts_per_div(), 1.0);
    assert_eq!(s.time_per_div(), 500e-6);
}

#[test]
fn zero_calibration_on_the_fake_scope() {
    let dir = std::env::temp_dir().join(format!("oht-e2e-zero-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("config.toml");
    let loaded = openhantek_tui::config::load(&path, None);
    let (mut app, fake) = live_app([Wave::Dc(0.0), Wave::Dc(0.0)], loaded.settings);
    app.save_target = loaded.save;
    // The 30 MS/s bank of this scope reads 3.5 counts high (not in its
    // EEPROM): CH1 alone shows 0 V as +0.14 V at 1 V/div.
    fake.lock().fast_zero_shift = [3.5, 0.0];
    tick_until(&mut app, 5.0, |a| a.connected);
    let before = app.scope.settings.clone();
    // Z asks first; any key but y says no.
    app.handle_key(key('Z'));
    assert!(app.ask_zero_cal);
    app.handle_key(key('n'));
    assert!(!app.ask_zero_cal && app.zero_cal.is_none());
    app.handle_key(key('Z'));
    app.handle_key(key('y'));
    assert!(app.zero_cal.is_some());
    tick_until(&mut app, 30.0, |a| a.zero_cal.is_none());
    let m = app.current_message().unwrap().text.clone();
    assert!(m.contains("Zero calibration done: 24 values"), "{m}");
    assert!(m.contains("saved in the config file"), "{m}");
    assert!(m.contains("Biggest change +3."), "{m}");
    assert!(m.contains("at 30 MS/s)"), "{m}");
    let cal = &app.scope.settings.calibration;
    for g in 0..8 {
        assert!((cal.zero_ch1[g] - 126.0).abs() < 0.2, "{cal:?}");
        assert!((cal.zero_ch2[g] - 131.0).abs() < 0.2, "{cal:?}");
        assert!((cal.zero_ch1_30m[g] - 129.5).abs() < 0.2, "{cal:?}");
    }
    // The user's settings are back, with the new calibration.
    let mut back = app.scope.settings.clone();
    back.calibration = before.calibration.clone();
    assert_eq!(back, before);
    // Saved in the config file at once.
    let text = std::fs::read_to_string(&path).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    let z = t["calibration"]["zero_ch1_30m"].as_array().unwrap();
    assert!((z[5].as_float().unwrap() - 129.5).abs() < 0.2, "{text}");
    // CH1 alone at 30 MS/s now reads 0 V.
    app.scope.settings.channels[1].enabled = false;
    app.scope.settings.channels[0].offset = 0.0;
    app.tick();
    assert!(app.scope.hw.unwrap().high_speed());
    app.scope.frame = None;
    tick_until(&mut app, 10.0, |a| a.scope.measure(0).is_some());
    let mean = app.scope.measure(0).unwrap().mean;
    assert!(mean.abs() < 0.02, "{mean}");
    // Clear: the EEPROM values again (0.14 V high at 30 MS/s).
    app.action(openhantek_tui::panel::Action::ClearZeroCal);
    assert_eq!(app.scope.settings.calibration.zero_count(), 0);
    let text = std::fs::read_to_string(&path).unwrap();
    let t: toml::Table = toml::from_str(&text).unwrap();
    let z = t["calibration"]["zero_ch1_30m"].as_array().unwrap();
    assert_eq!(z[5].as_float(), Some(0.0));
    std::fs::remove_dir_all(&dir).unwrap();
}

#[test]
fn zero_calibration_fails_with_a_signal_and_restores_the_settings() {
    let (mut app, _fake) = live_app([SINE_1K, Wave::Dc(0.0)], Settings::default());
    tick_until(&mut app, 5.0, |a| a.connected);
    let before = app.scope.settings.clone();
    app.handle_key(key('Z'));
    app.handle_key(key('y'));
    tick_until(&mut app, 30.0, |a| a.zero_cal.is_none());
    let m = app.current_message().unwrap();
    // 3 Vpp at 20 mV/div: out of range (a smaller one: "has a signal").
    assert!(m.error && m.text.contains("CH1 is out of range"), "{m:?}");
    assert_eq!(app.scope.settings, before);
    assert_eq!(app.scope.settings.calibration.zero_count(), 0);
}

#[test]
fn probe_helper_on_the_calibration_output() {
    use ratatui::crossterm::event::{KeyCode, KeyEvent};
    let dir = std::env::temp_dir().join(format!("oht-e2e-probe-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("config.toml");
    let loaded = openhantek_tui::config::load(&path, None);
    // 1.96 Vpp: the input reads 2 % low.
    let low = Wave::Square {
        freq: 1000.0,
        low: 0.0,
        high: 1.96,
        duty: 0.5,
    };
    let (mut app, fake) = live_app([Wave::CalOut, Wave::Dc(0.0)], loaded.settings);
    app.save_target = loaded.save;
    tick_until(&mut app, 5.0, |a| a.connected);
    app.scope.settings.time_idx = time_index(10e-3);
    let before = app.scope.settings.clone();
    app.handle_key(key('C'));
    assert!(app.probe_cal.is_some());
    let s = &app.scope.settings;
    assert_eq!(s.channels[0].volts_per_div(), 0.5);
    assert_eq!(s.time_per_div(), 200e-6);
    assert!(!s.channels[1].enabled);
    tick_until(&mut app, 10.0, |a| {
        a.probe_cal.as_ref().unwrap().reading.is_some()
    });
    let r = app.probe_cal.as_ref().unwrap().reading.unwrap();
    assert!(r.edge.abs() < 1.0, "{r:?}");
    assert!(r.gain_error().abs() < 1.0, "{r:?}");
    assert!((r.freq.unwrap() - 1000.0).abs() < 5.0, "{r:?}");
    // The box shows it.
    let mut term = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 40)).unwrap();
    term.draw(|f| openhantek_tui::ui::draw(f, &app)).unwrap();
    let text: String = term
        .backend()
        .buffer()
        .content()
        .iter()
        .map(|c| c.symbol())
        .collect();
    assert!(
        text.contains("Probe helper") && text.contains("Flat top"),
        "{text}"
    );
    // A frequency step: 800 Hz, 500 us/div.
    app.handle_key(KeyEvent::from(KeyCode::Left));
    assert_eq!(app.scope.settings.cal_freq, 800);
    assert_eq!(app.scope.settings.time_per_div(), 500e-6);
    // 2 % low: Enter saves a gain correction of 2 / 1.96.
    fake.lock().waves[0] = low;
    app.handle_key(KeyEvent::from(KeyCode::Right));
    tick_until(&mut app, 10.0, |a| {
        a.probe_cal
            .as_ref()
            .unwrap()
            .reading
            .is_some_and(|r| r.gain_error() < -1.5)
    });
    app.handle_key(KeyEvent::from(KeyCode::Enter));
    let g = app.scope.settings.calibration.gain[0];
    assert!((g - 2.0 / 1.96).abs() < 0.003, "{g}");
    assert!(app
        .current_message()
        .unwrap()
        .text
        .contains("saved in the config file"));
    // Measured again with the correction: 2 V.
    tick_until(&mut app, 10.0, |a| {
        a.probe_cal
            .as_ref()
            .unwrap()
            .reading
            .is_some_and(|r| r.gain_error().abs() < 0.3)
    });
    // Esc: the user's settings are back, with the gain correction.
    app.handle_key(KeyEvent::from(KeyCode::Esc));
    assert!(app.probe_cal.is_none());
    let mut back = app.scope.settings.clone();
    back.calibration = before.calibration.clone();
    assert_eq!(back, before);
    let t: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    let saved = t["calibration"]["gain"].as_array().unwrap()[0]
        .as_float()
        .unwrap();
    assert!((saved - g).abs() < 1e-9);
    // Clear it from the panel action.
    app.action(openhantek_tui::panel::Action::ClearGainCal);
    assert_eq!(app.scope.settings.calibration.gain, [1.0, 1.0]);
    std::fs::remove_dir_all(&dir).unwrap();
}

fn key(c: char) -> ratatui::crossterm::event::KeyEvent {
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let code = match c {
        '→' => KeyCode::Right,
        '←' => KeyCode::Left,
        '↑' => KeyCode::Up,
        '↓' => KeyCode::Down,
        c => KeyCode::Char(c),
    };
    KeyEvent::new(code, KeyModifiers::NONE)
}

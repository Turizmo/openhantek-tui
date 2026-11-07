//! Tests with the real scope. Run with:
//! `cargo test --test hardware -- --ignored --test-threads=1 --nocapture`
//! Close OpenHantek6022 first. A probe on the calibration pin makes the
//! signal checks stronger; without it they only check that capture works.

use openhantek_tui::device::protocol::{choose_rate, gain_index, HwConfig, BLOCK_SAMPLES};
use openhantek_tui::device::{open_hardware, Connection};
use openhantek_tui::dsp::measure::measure;

fn open() -> Connection {
    open_hardware(None, &mut |s| eprintln!("status: {s}")).expect("open the real scope")
}

fn config(time_per_div: f64, volts_per_div: f64, channels: u8, cal_freq: u32) -> HwConfig {
    let g = gain_index(volts_per_div);
    HwConfig {
        rate: choose_rate(time_per_div * 10.0, channels == 2),
        channels,
        gains: [g, g],
        dc: [true; 2],
        cal_freq,
        roll: false,
    }
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_capture_two_channels() {
    let mut c = open();
    eprintln!("opened {} notes {:?}", c.name, c.notes);
    let cfg = config(1e-3, 1.0, 2, 1000);
    // First block after a config change can be odd: take a few.
    let mut last = None;
    for _ in 0..3 {
        last = Some(c.driver.capture(&cfg).unwrap());
    }
    let block = last.unwrap();
    assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
    assert_eq!(block.data[1].len(), BLOCK_SAMPLES);
    for ch in 0..2 {
        let m = measure(&block.data[ch], 1.0 / block.rate);
        eprintln!(
            "CH{} min {:.3} max {:.3} mean {:.3} vpp {:.3} freq {:?} duty {:?} clipped {}",
            ch + 1,
            m.min,
            m.max,
            m.mean,
            m.pk_pk,
            m.freq,
            m.duty,
            block.clipped[ch]
        );
        // Inputs must read inside the ADC range.
        assert!(m.min > -5.3 && m.max < 5.3);
        if m.pk_pk > 1.0 {
            // A probe is on the calibration pin: 1 kHz square, about 2 Vpp.
            let f = m.freq.expect("a frequency on a 2 Vpp signal");
            assert!((f - 1000.0).abs() < 20.0, "freq {f}");
            assert!((m.pk_pk - 2.0).abs() < 0.5, "vpp {}", m.pk_pk);
        } else {
            eprintln!("CH{}: flat signal, only capture checked", ch + 1);
        }
    }
    c.driver.stop().unwrap();
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_calibration_frequency_follows_setting() {
    let mut c = open();
    let mut freqs = Vec::new();
    for hz in [1000u32, 2000, 500] {
        let cfg = config(1e-3, 1.0, 2, hz);
        let mut block = None;
        for _ in 0..3 {
            block = Some(c.driver.capture(&cfg).unwrap());
        }
        let block = block.unwrap();
        let m = measure(&block.data[0], 1.0 / block.rate);
        eprintln!("cal {hz} Hz: CH1 vpp {:.3} freq {:?}", m.pk_pk, m.freq);
        if m.pk_pk < 1.0 {
            eprintln!("CH1 flat: no probe on the calibration pin, skipping the check");
            c.driver.stop().unwrap();
            return;
        }
        freqs.push((hz, m.freq.unwrap()));
    }
    for (want, got) in freqs {
        assert!(
            (got - want as f64).abs() < want as f64 * 0.02,
            "{want} Hz: got {got}"
        );
    }
    c.driver.stop().unwrap();
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_fast_single_channel_and_gains() {
    let mut c = open();
    for (tdiv, vdiv) in [(20e-6, 0.5), (100e-6, 0.1), (10e-3, 2.0)] {
        let cfg = config(tdiv, vdiv, 1, 1000);
        let mut block = None;
        for _ in 0..2 {
            block = Some(c.driver.capture(&cfg).unwrap());
        }
        let block = block.unwrap();
        let m = measure(&block.data[0], 1.0 / block.rate);
        eprintln!(
            "{tdiv} s/div {vdiv} V/div rate {} S/s: min {:.3} max {:.3} freq {:?} clipped {}",
            block.rate, m.min, m.max, m.freq, block.clipped[0]
        );
        assert_eq!(block.data[0].len(), BLOCK_SAMPLES);
        assert!(block.data[1].is_empty());
    }
    c.driver.stop().unwrap();
}

#[test]
#[ignore = "needs the real DSO-6022BE"]
fn hw_roll_stream() {
    let mut c = open();
    // 200 ms/div: 5 kS/s. A 100 Hz calibration signal has 50 samples per period.
    let mut cfg = config(200e-3, 1.0, 2, 100);
    cfg.rate = openhantek_tui::device::protocol::roll_rate(2.0);
    cfg.roll = true;
    let mut ch1 = Vec::new();
    let t0 = std::time::Instant::now();
    while t0.elapsed().as_secs_f64() < 2.0 {
        let piece = c.driver.stream(&cfg).unwrap();
        ch1.extend_from_slice(&piece.data[0]);
    }
    let secs = t0.elapsed().as_secs_f64();
    eprintln!(
        "roll: {} samples in {secs:.2} s at {} S/s",
        ch1.len(),
        cfg.rate.rate
    );
    assert!(
        ch1.len() as f64 > cfg.rate.rate * secs * 0.8,
        "only {} samples",
        ch1.len()
    );
    let m = measure(&ch1, 1.0 / cfg.rate.rate);
    eprintln!("roll CH1 vpp {:.3} freq {:?}", m.pk_pk, m.freq);
    if m.pk_pk > 1.0 {
        // Lost samples would make the frequency read high.
        let f = m.freq.unwrap();
        assert!((f - 100.0).abs() < 0.5, "roll freq {f}");
    }
    c.driver.stop().unwrap();
}

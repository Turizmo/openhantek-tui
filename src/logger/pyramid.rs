//! Summaries of a log for fast drawing when zoomed out: level 0 has one
//! entry (min of mins, max of maxes, mean of means per channel) for each
//! [`BASE`] steps, and each level above has one entry for [`FACTOR`]
//! entries of the level below. Built step by step from the file (so a big
//! file opens at once and fills in), kept up to date while a log grows,
//! and cached in a sidecar file (`<log>.idx`).
//!
//! A range is summed up exactly: whole entries of the highest level that
//! fit, the lower levels at its ends, and the raw records only for the
//! ends that are shorter than [`BASE`] steps.

use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::format::Rec;
use super::reader::LogFile;

/// Steps per entry of level 0.
pub const BASE: u64 = 1024;
/// Entries of a level per entry of the level above.
pub const FACTOR: u64 = 16;

/// Min, max and mean of one channel over some steps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Agg {
    pub min: f32,
    pub max: f32,
    pub mean: f32,
    /// Steps with data.
    pub n: u32,
}

impl Agg {
    pub const EMPTY: Agg = Agg {
        min: f32::INFINITY,
        max: f32::NEG_INFINITY,
        mean: 0.0,
        n: 0,
    };

    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn add(&mut self, r: [f32; 3]) {
        if r[2].is_nan() {
            return;
        }
        self.merge(&Agg {
            min: r[0],
            max: r[1],
            mean: r[2],
            n: 1,
        });
    }

    /// Mean of means, weighted by the steps.
    pub fn merge(&mut self, o: &Agg) {
        if o.n == 0 {
            return;
        }
        let n = self.n as f64 + o.n as f64;
        self.mean = ((self.mean as f64 * self.n as f64 + o.mean as f64 * o.n as f64) / n) as f32;
        self.n = self.n.saturating_add(o.n);
        self.min = self.min.min(o.min);
        self.max = self.max.max(o.max);
    }
}

/// CH1 and CH2.
pub type Aggs = [Agg; 2];
pub const EMPTY: Aggs = [Agg::EMPTY; 2];

fn add_rec(a: &mut Aggs, r: &Rec) {
    a[0].add(r[0]);
    a[1].add(r[1]);
}

fn merge(a: &mut Aggs, b: &Aggs) {
    a[0].merge(&b[0]);
    a[1].merge(&b[1]);
}

#[derive(Clone, Debug, Default)]
pub struct Pyramid {
    /// `levels[k]` has one entry per `BASE * FACTOR^k` steps, complete
    /// entries only.
    pub levels: Vec<Vec<Aggs>>,
    /// Steps summed into level 0 (a multiple of [`BASE`]).
    pub covered: u64,
}

/// Steps per entry of level `k`.
pub fn bin(k: usize) -> u64 {
    BASE * FACTOR.pow(k as u32)
}

impl Pyramid {
    /// Sum up more of the file: at most `budget` steps. True when the
    /// levels changed.
    pub fn update(&mut self, f: &LogFile, budget: u64) -> Result<bool> {
        let end = f.end() / BASE * BASE;
        if self.covered >= end {
            return Ok(false);
        }
        let stop = end.min(self.covered + budget.max(BASE) / BASE * BASE);
        let mut entry = EMPTY;
        let mut at = self.covered;
        let mut out = Vec::new();
        // Records come in step order; empty steps (gaps) just add nothing.
        let push_until = |i: u64, entry: &mut Aggs, at: &mut u64, out: &mut Vec<Aggs>| {
            while *at + BASE <= i {
                out.push(*entry);
                *entry = EMPTY;
                *at += BASE;
            }
        };
        f.for_each(self.covered..stop, |first, recs| {
            for (k, r) in recs.iter().enumerate() {
                let i = first + k as u64;
                push_until(i, &mut entry, &mut at, &mut out);
                add_rec(&mut entry, r);
            }
        })?;
        push_until(stop, &mut entry, &mut at, &mut out);
        self.covered = stop;
        for e in out {
            self.push(0, e);
        }
        Ok(true)
    }

    fn push(&mut self, k: usize, e: Aggs) {
        if self.levels.len() <= k {
            self.levels.push(Vec::new());
        }
        self.levels[k].push(e);
        let n = self.levels[k].len() as u64;
        if n.is_multiple_of(FACTOR) {
            let mut up = EMPTY;
            for x in &self.levels[k][(n - FACTOR) as usize..] {
                merge(&mut up, x);
            }
            self.push(k + 1, up);
        }
    }

    /// True when the whole file is summed up (up to its last whole
    /// [`BASE`] steps).
    pub fn complete(&self, f: &LogFile) -> bool {
        self.covered >= f.end() / BASE * BASE
    }

    /// Min, max and mean of each channel over the steps in `range`.
    pub fn agg(&self, f: &LogFile, range: Range<u64>) -> Result<Aggs> {
        let mut out = EMPTY;
        let mut raw: Option<u64> = None;
        let mut i = range.start;
        let b = range.end;
        while i < b {
            let level = (0..self.levels.len()).rev().find(|&k| {
                let w = bin(k);
                i.is_multiple_of(w) && i + w <= b && ((i / w) as usize) < self.levels[k].len()
            });
            match level {
                Some(k) => {
                    if let Some(r) = raw.take() {
                        raw_agg(f, r..i, &mut out)?;
                    }
                    merge(&mut out, &self.levels[k][(i / bin(k)) as usize]);
                    i += bin(k);
                }
                None => {
                    raw.get_or_insert(i);
                    i = ((i / BASE + 1) * BASE).min(b);
                }
            }
        }
        if let Some(r) = raw {
            raw_agg(f, r..b, &mut out)?;
        }
        Ok(out)
    }

    /// `n` columns of `z` steps each, from step `start`.
    pub fn columns(&self, f: &LogFile, start: u64, z: u64, n: usize) -> Result<Vec<Aggs>> {
        let z = z.max(1);
        if z < BASE {
            // Few steps per column: one read for all of them.
            let mut cols = vec![EMPTY; n];
            let end = start + z * n as u64;
            f.for_each(start..end, |first, recs| {
                for (k, r) in recs.iter().enumerate() {
                    let c = ((first + k as u64 - start) / z) as usize;
                    if let Some(col) = cols.get_mut(c) {
                        add_rec(col, r);
                    }
                }
            })?;
            return Ok(cols);
        }
        (0..n as u64)
            .map(|c| self.agg(f, start + c * z..start + (c + 1) * z))
            .collect()
    }

    /// Sidecar cache file of a log.
    pub fn sidecar(log: &Path) -> PathBuf {
        let mut s = log.as_os_str().to_owned();
        s.push(".idx");
        PathBuf::from(s)
    }

    /// Save the levels next to the log.
    pub fn save(&self, f: &LogFile) -> Result<()> {
        let mut b = Vec::new();
        b.extend_from_slice(IDX_MAGIC);
        b.extend_from_slice(&f.header.start_ns.to_le_bytes());
        b.extend_from_slice(&f.header.step_ns.to_le_bytes());
        b.extend_from_slice(&(f.header.record_bytes() as u64).to_le_bytes());
        b.extend_from_slice(&self.covered.to_le_bytes());
        b.extend_from_slice(&(self.levels.len() as u64).to_le_bytes());
        for l in &self.levels {
            b.extend_from_slice(&(l.len() as u64).to_le_bytes());
            for e in l {
                for a in e {
                    for v in [a.min, a.max, a.mean] {
                        b.extend_from_slice(&v.to_le_bytes());
                    }
                    b.extend_from_slice(&a.n.to_le_bytes());
                }
            }
        }
        let path = Self::sidecar(&f.path);
        let tmp = path.with_extension("idx.tmp");
        std::fs::write(&tmp, b)?;
        std::fs::rename(&tmp, &path)?;
        Ok(())
    }

    /// Load the sidecar cache, if it is there and fits the log.
    pub fn load(f: &LogFile) -> Option<Pyramid> {
        let b = std::fs::read(Self::sidecar(&f.path)).ok()?;
        let mut at = 0usize;
        let mut take = |n: usize| -> Option<Vec<u8>> {
            let s = b.get(at..at + n)?.to_vec();
            at += n;
            Some(s)
        };
        if take(8)? != IDX_MAGIC {
            return None;
        }
        let mut u64_ = || -> Option<u64> { Some(u64::from_le_bytes(take(8)?.try_into().ok()?)) };
        let start = u64_()? as i64;
        let step = u64_()?;
        let rec = u64_()?;
        let covered = u64_()?;
        if start != f.header.start_ns
            || step != f.header.step_ns
            || rec != f.header.record_bytes() as u64
            || covered > f.end()
        {
            return None;
        }
        let nl = u64_()? as usize;
        let mut levels = Vec::new();
        for _ in 0..nl.min(64) {
            let n = u64::from_le_bytes(take(8)?.try_into().ok()?) as usize;
            let bytes = take(n.checked_mul(32)?)?;
            let f32_at = |o: usize| f32::from_le_bytes(bytes[o..o + 4].try_into().unwrap());
            let l = (0..n)
                .map(|i| {
                    [0, 1].map(|c| {
                        let o = i * 32 + c * 16;
                        Agg {
                            min: f32_at(o),
                            max: f32_at(o + 4),
                            mean: f32_at(o + 8),
                            n: u32::from_le_bytes(bytes[o + 12..o + 16].try_into().unwrap()),
                        }
                    })
                })
                .collect();
            levels.push(l);
        }
        let p = Pyramid { levels, covered };
        (p.levels.first().map_or(0, |l| l.len() as u64) * BASE == covered).then_some(p)
    }
}

const IDX_MAGIC: &[u8; 8] = b"OHTIDX01";

fn raw_agg(f: &LogFile, range: Range<u64>, out: &mut Aggs) -> Result<()> {
    f.for_each(range, |_, recs| {
        for r in recs {
            add_rec(out, r);
        }
    })
}

#[cfg(test)]
mod tests {
    use super::super::format::{test_header, ChunkHead, OPEN};
    use super::*;

    /// A CH1+CH2 log with steps `0..n` minus a gap, values from `f(i)`.
    fn log(path: &Path, n: u64, gap: Range<u64>, f: impl Fn(u64) -> f32) -> LogFile {
        let h = test_header([true, true], 1e-3);
        let mut b = h.encode();
        for (k, r) in [0..gap.start, gap.end..n].iter().enumerate() {
            b.extend_from_slice(
                &ChunkHead {
                    flags: k as u32,
                    first: r.start,
                    count: if k == 0 { r.end - r.start } else { OPEN },
                }
                .encode(),
            );
            for i in r.clone() {
                let v = f(i);
                for x in [v - 1.0, v + 1.0, v, -v, -v, -v] {
                    b.extend_from_slice(&x.to_le_bytes());
                }
            }
        }
        std::fs::write(path, b).unwrap();
        LogFile::open(path).unwrap()
    }

    fn brute(n: u64, gap: &Range<u64>, r: Range<u64>, f: impl Fn(u64) -> f32) -> Aggs {
        let mut a = EMPTY;
        for i in r {
            if i < n && !gap.contains(&i) {
                let v = f(i);
                add_rec(&mut a, &[[v - 1.0, v + 1.0, v], [-v; 3]]);
            }
        }
        a
    }

    fn close(a: &Aggs, b: &Aggs) -> bool {
        (0..2).all(|c| {
            a[c].n == b[c].n
                && (a[c].n == 0
                    || (a[c].min == b[c].min
                        && a[c].max == b[c].max
                        && (a[c].mean - b[c].mean).abs() <= 1e-3 * (1.0 + b[c].mean.abs())))
        })
    }

    #[test]
    fn pyramid_matches_brute_force() {
        let dir = std::env::temp_dir().join(format!("oht-pyr-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("p.ohlog");
        let val = |i: u64| ((i * 7919) % 1000) as f32 / 100.0 + (i as f32 / 50_000.0).sin();
        let n = 300_000u64;
        let gap = 40_000..61_000u64;
        let f = log(&path, n, gap.clone(), val);
        let mut p = Pyramid::default();
        // In small pieces, like the view does it.
        while p.update(&f, 50_000).unwrap() {}
        assert!(p.complete(&f));
        assert_eq!(p.covered, n / BASE * BASE);
        assert_eq!(p.levels[0].len() as u64, n / BASE);
        assert_eq!(p.levels[1].len() as u64, n / BASE / FACTOR);
        assert!(p.levels[0][45][0].is_empty()); // inside the gap
        for r in [
            0..n,
            1..n - 1,
            1000..1001,
            BASE..2 * BASE,
            12_345..250_000,
            39_000..62_000,
            41_000..60_000,
            299_000..400_000,
        ] {
            let got = p.agg(&f, r.clone()).unwrap();
            let want = brute(n, &gap, r.clone(), val);
            assert!(close(&got, &want), "{r:?}: {got:?} {want:?}");
        }
        // Columns, below and above BASE steps each.
        for z in [3u64, 1500, 20_000] {
            let cols = p.columns(&f, 500, z, 14).unwrap();
            for (c, got) in cols.iter().enumerate() {
                let s = 500 + c as u64 * z;
                let want = brute(n, &gap, s..s + z, val);
                assert!(close(got, &want), "z {z} col {c}");
            }
        }
        // The sidecar cache gives the same levels.
        p.save(&f).unwrap();
        let q = Pyramid::load(&f).unwrap();
        assert_eq!(q.covered, p.covered);
        assert_eq!(q.levels, p.levels);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

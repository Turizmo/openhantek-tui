//! Reading a log file, also while the writer is still adding to it.

use std::fs::File;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::format::{decode_records, ChunkHead, Header, Rec, CHUNK_HEADER, OPEN};

/// One chunk of records: steps `first .. first + count`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Chunk {
    pub first: u64,
    pub count: u64,
    /// File offset of the chunk head.
    pub head_at: u64,
    /// The count is not written yet: the records go to the end of the file.
    pub open: bool,
    /// Data was lost before this chunk.
    pub gap: bool,
}

impl Chunk {
    pub fn end(&self) -> u64 {
        self.first + self.count
    }

    fn data_at(&self) -> u64 {
        self.head_at + CHUNK_HEADER as u64
    }
}

pub struct LogFile {
    pub path: PathBuf,
    file: File,
    pub header: Header,
    header_len: u64,
    pub chunks: Vec<Chunk>,
    len: u64,
}

/// Records read in one go when going through a long range.
const READ_RECORDS: u64 = 1 << 16;

impl LogFile {
    pub fn open(path: &Path) -> Result<LogFile> {
        let file = File::open(path).with_context(|| format!("cannot open {}", path.display()))?;
        let mut head = vec![0u8; 4096];
        let n = read_some(&file, &mut head, 0)?;
        let (header, header_len) = Header::decode(&head[..n])
            .with_context(|| format!("cannot read {}", path.display()))?;
        let mut f = LogFile {
            path: path.to_path_buf(),
            file,
            header,
            header_len: header_len as u64,
            chunks: Vec::new(),
            len: 0,
        };
        f.refresh()?;
        Ok(f)
    }

    fn rec(&self) -> u64 {
        self.header.record_bytes() as u64
    }

    /// Look for data the writer added. True when there are new records.
    pub fn refresh(&mut self) -> Result<bool> {
        let before = (self.end(), self.chunks.len());
        self.len = self.file.metadata()?.len();
        let mut at = self.header_len;
        if let Some(last) = self.chunks.pop() {
            if last.open {
                at = last.head_at;
            } else {
                at = last.data_at() + last.count * self.rec();
                self.chunks.push(last);
            }
        }
        let rec = self.rec();
        loop {
            let mut b = [0u8; CHUNK_HEADER];
            if at + CHUNK_HEADER as u64 > self.len || read_some(&self.file, &mut b, at)? < b.len() {
                break;
            }
            let Some(h) = ChunkHead::decode(&b) else {
                break;
            };
            let data_at = at + CHUNK_HEADER as u64;
            let room = (self.len - data_at) / rec;
            let open = h.count == OPEN;
            let count = if open { room } else { h.count.min(room) };
            self.chunks.push(Chunk {
                first: h.first,
                count,
                head_at: at,
                open: open || count < h.count,
                gap: h.gap(),
            });
            if open || count < h.count {
                break;
            }
            at = data_at + count * rec;
        }
        Ok((self.end(), self.chunks.len()) != before)
    }

    /// Step after the last record.
    pub fn end(&self) -> u64 {
        self.chunks.last().map_or(0, |c| c.end())
    }

    /// First step with data.
    pub fn first(&self) -> u64 {
        self.chunks.first().map_or(0, |c| c.first)
    }

    /// Records in the file (gaps not counted).
    pub fn records(&self) -> u64 {
        self.chunks.iter().map(|c| c.count).sum()
    }

    /// Gaps as step ranges `(from, to)`: no data from `from` to `to`. A
    /// gap shorter than a step has `from == to`.
    pub fn gaps(&self) -> Vec<(u64, u64)> {
        self.chunks
            .windows(2)
            .filter(|w| w[1].gap || w[1].first != w[0].end())
            .map(|w| (w[0].end(), w[1].first.max(w[0].end())))
            .collect()
    }

    /// Seconds since the start of step `i`.
    pub fn time_of(&self, i: f64) -> f64 {
        i * self.header.step()
    }

    /// Call `f` with each run of records in `range`, in order: the step
    /// of the first record and the records.
    pub fn for_each(&self, range: Range<u64>, mut f: impl FnMut(u64, &[Rec])) -> Result<()> {
        let rec = self.rec();
        let mut bytes = Vec::new();
        let mut recs = Vec::new();
        for c in &self.chunks {
            let a = range.start.max(c.first);
            let b = range.end.min(c.end());
            let mut i = a;
            while i < b {
                let n = (b - i).min(READ_RECORDS);
                bytes.resize((n * rec) as usize, 0);
                let at = c.data_at() + (i - c.first) * rec;
                let got = read_some(&self.file, &mut bytes, at)?;
                recs.clear();
                decode_records(&self.header, &bytes[..got], &mut recs);
                if recs.is_empty() {
                    break;
                }
                f(i, &recs);
                i += recs.len() as u64;
            }
        }
        Ok(())
    }

    /// The records in `range`, with their steps.
    pub fn read(&self, range: Range<u64>) -> Result<Vec<(u64, Rec)>> {
        let mut out = Vec::new();
        self.for_each(range, |first, r| {
            out.extend(r.iter().enumerate().map(|(k, x)| (first + k as u64, *x)));
        })?;
        Ok(out)
    }
}

/// Read as much as there is at `at`, up to `buf.len()`.
fn read_some(f: &File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match f.read_at(&mut buf[n..], at + n as u64) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::super::format::{test_header, FLAG_GAP};
    use super::*;

    /// A file with chunks of (first, values, closed).
    pub(crate) fn write_log(path: &Path, h: &Header, chunks: &[(u64, Vec<f32>, bool)]) {
        let mut b = h.encode();
        let per = h.record_bytes() / 4;
        for (k, (first, v, closed)) in chunks.iter().enumerate() {
            let count = if *closed {
                (v.len() / per) as u64
            } else {
                OPEN
            };
            let flags = if k > 0 { FLAG_GAP } else { 0 };
            b.extend_from_slice(
                &ChunkHead {
                    flags,
                    first: *first,
                    count,
                }
                .encode(),
            );
            for x in v {
                b.extend_from_slice(&x.to_le_bytes());
            }
        }
        std::fs::write(path, b).unwrap();
    }

    #[test]
    fn follows_a_growing_file() {
        let dir = std::env::temp_dir().join(format!("oht-reader-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("g.ohlog");
        let h = test_header([true, false], 2.5e-6);
        write_log(&path, &h, &[(0, vec![1.0, 2.0], false)]);
        let mut f = LogFile::open(&path).unwrap();
        assert_eq!(f.end(), 2);
        assert!(!f.refresh().unwrap());
        // The writer adds records, then closes the chunk and starts a new
        // one after a gap.
        write_log(
            &path,
            &h,
            &[(0, vec![1.0, 2.0, 3.0], true), (5, vec![7.0], false)],
        );
        assert!(f.refresh().unwrap());
        assert_eq!(f.end(), 6);
        assert_eq!(f.records(), 4);
        assert_eq!(f.gaps(), vec![(3, 5)]);
        let r = f.read(2..6).unwrap();
        assert_eq!(r.iter().map(|x| x.0).collect::<Vec<_>>(), vec![2, 5]);
        assert_eq!(r[1].1[0], [7.0; 3]);
        assert!((f.time_of(4.0) - 10e-6).abs() < 1e-12);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}

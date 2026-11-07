//! Load the firmware into the FX2LP RAM.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use super::hex::{self, Record};
use super::protocol::{CPUCS, FIRMWARE_CHUNK, REQ_RW_INTERNAL};
use super::transport::Transport;

/// File name we look for in the config folder.
pub const FIRMWARE_FILE: &str = "dso6022be-firmware.hex";

/// Places to look for the firmware, in order: the `--firmware` flag, the
/// path set at build time, then the config folder.
pub fn candidates(flag: Option<&Path>) -> Vec<PathBuf> {
    let mut list = Vec::new();
    if let Some(p) = flag {
        list.push(p.to_path_buf());
    }
    if let Some(p) = option_env!("OPENHANTEK_TUI_FIRMWARE") {
        list.push(PathBuf::from(p));
    }
    if let Some(dir) = crate::config::config_dir() {
        list.push(dir.join(FIRMWARE_FILE));
    }
    list
}

/// Read and parse the first firmware file that exists.
pub fn load(flag: Option<&Path>) -> Result<(PathBuf, Vec<Record>)> {
    let list = candidates(flag);
    // A path given with --firmware must work: do not fall back silently.
    if let Some(p) = flag {
        let text = std::fs::read_to_string(p)
            .with_context(|| format!("cannot read firmware {}", p.display()))?;
        return Ok((p.to_path_buf(), hex::parse(&text)?));
    }
    for p in &list {
        if let Ok(text) = std::fs::read_to_string(p) {
            let records =
                hex::parse(&text).with_context(|| format!("bad firmware file {}", p.display()))?;
            return Ok((p.clone(), records));
        }
    }
    let tried: Vec<String> = list.iter().map(|p| p.display().to_string()).collect();
    bail!(
        "the scope needs firmware, but no firmware file was found. Tried: {}. \
         Get dso6022be-firmware.hex from OpenHantek6022 (openhantek/res/firmware/) \
         and pass it with --firmware",
        tried.join(", ")
    )
}

/// Join touching records into chunks of at most [`FIRMWARE_CHUNK`] bytes.
pub fn chunks(records: &[Record]) -> Vec<Record> {
    let mut out: Vec<Record> = Vec::new();
    for rec in records {
        let mut addr = rec.addr;
        for piece in rec.data.chunks(FIRMWARE_CHUNK) {
            match out.last_mut() {
                Some(last)
                    if last.addr + last.data.len() as u32 == addr
                        && last.data.len() + piece.len() <= FIRMWARE_CHUNK =>
                {
                    last.data.extend_from_slice(piece);
                }
                _ => out.push(Record {
                    addr,
                    data: piece.to_vec(),
                }),
            }
            addr += piece.len() as u32;
        }
    }
    out
}

/// Check that all records fit the FX2LP internal RAM (0x0000..0x3FFF,
/// 0xE000..0xE1FF).
pub fn check_ram(records: &[Record]) -> Result<()> {
    for r in records {
        let end = r.addr as usize + r.data.len();
        let ok = end <= 0x4000 || (r.addr >= 0xE000 && end <= 0xE200);
        if !ok {
            bail!("firmware writes outside the FX2LP RAM at {:#06x}", r.addr);
        }
    }
    Ok(())
}

/// Upload: hold the CPU in reset, write the RAM, let the CPU run. The
/// device then drops off the bus and comes back with the new firmware.
pub fn upload(t: &mut impl Transport, records: &[Record]) -> Result<()> {
    check_ram(records)?;
    t.control_out(REQ_RW_INTERNAL, CPUCS, 0, &[0x01])
        .context("cannot stop the scope CPU for the firmware upload")?;
    for c in chunks(records) {
        t.control_out(
            REQ_RW_INTERNAL,
            (c.addr & 0xFFFF) as u16,
            (c.addr >> 16) as u16,
            &c.data,
        )
        .with_context(|| format!("firmware write at {:#06x} failed", c.addr))?;
    }
    // The device may drop off the bus before it answers: that is fine.
    let _ = t.control_out(REQ_RW_INTERNAL, CPUCS, 0, &[0x00]);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::device::transport::{Recorder, VENDOR_OUT};

    fn rec(addr: u32, len: usize) -> Record {
        Record {
            addr,
            data: (0..len).map(|i| i as u8).collect(),
        }
    }

    #[test]
    fn chunks_join_touching_records() {
        let c = chunks(&[rec(0, 16), rec(16, 16), rec(100, 4)]);
        assert_eq!(c.len(), 2);
        assert_eq!(c[0].addr, 0);
        assert_eq!(c[0].data.len(), 32);
        assert_eq!(c[1].addr, 100);
    }

    #[test]
    fn chunks_never_pass_the_limit() {
        let records: Vec<Record> = (0..100).map(|i| rec(i * 32, 32)).collect();
        let c = chunks(&records);
        assert!(c.iter().all(|r| r.data.len() <= FIRMWARE_CHUNK));
        let total: usize = c.iter().map(|r| r.data.len()).sum();
        assert_eq!(total, 3200);
        // A big record is split too.
        let c = chunks(&[rec(0, 3000)]);
        assert_eq!(
            c.iter().map(|r| r.data.len()).collect::<Vec<_>>(),
            vec![1023, 1023, 954]
        );
        assert_eq!(c[1].addr, 1023);
    }

    #[test]
    fn upload_sequence() {
        let mut t = Recorder::default();
        upload(&mut t, &[rec(0x0000, 4), rec(0x3D00, 8)]).unwrap();
        let calls = &t.calls;
        assert_eq!(calls.len(), 4);
        assert!(calls
            .iter()
            .all(|c| c.request_type == VENDOR_OUT && c.request == 0xA0));
        assert_eq!((calls[0].value, calls[0].data.clone()), (0xE600, vec![1]));
        assert_eq!(calls[1].value, 0x0000);
        assert_eq!(calls[2].value, 0x3D00);
        assert_eq!(calls[2].data.len(), 8);
        assert_eq!((calls[3].value, calls[3].data.clone()), (0xE600, vec![0]));
    }

    #[test]
    fn upload_refuses_external_memory() {
        let mut t = Recorder::default();
        assert!(upload(&mut t, &[rec(0x4000, 4)]).is_err());
        assert!(t.calls.is_empty());
    }

    #[test]
    fn real_firmware_uploads_7042_bytes() {
        let Some(path) = option_env!("OPENHANTEK_TUI_FIRMWARE") else {
            return;
        };
        let (_, records) = load(Some(Path::new(path))).unwrap();
        let mut t = Recorder::default();
        upload(&mut t, &records).unwrap();
        let data: usize = t.calls[1..t.calls.len() - 1]
            .iter()
            .map(|c| c.data.len())
            .sum();
        assert_eq!(data, 7042);
    }

    #[test]
    fn missing_flag_file_is_an_error() {
        let err = load(Some(Path::new("/nonexistent/fw.hex"))).unwrap_err();
        assert!(err.to_string().contains("cannot read firmware"));
    }
}

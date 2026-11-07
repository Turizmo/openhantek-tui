//! Intel HEX parser for the scope firmware.

use anyhow::{bail, Context, Result};

/// One block of firmware bytes to write at `addr`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub addr: u32,
    pub data: Vec<u8>,
}

/// Parse Intel HEX text into data records, in file order.
/// Checks every checksum. Supports record types 00, 01, 02 and 04.
pub fn parse(text: &str) -> Result<Vec<Record>> {
    let mut records = Vec::new();
    let mut base: u32 = 0;
    let mut saw_eof = false;
    for (n, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() {
            continue;
        }
        let lineno = n + 1;
        let Some(hex) = line.strip_prefix(':') else {
            bail!("line {lineno}: does not start with ':'");
        };
        if hex.len() % 2 != 0 || hex.len() < 10 {
            bail!("line {lineno}: bad length");
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .with_context(|| format!("line {lineno}: not hex"))?;
        let len = bytes[0] as usize;
        if bytes.len() != len + 5 {
            bail!("line {lineno}: length byte says {len}, line has {}", bytes.len() - 5);
        }
        let sum = bytes.iter().fold(0u8, |a, b| a.wrapping_add(*b));
        if sum != 0 {
            bail!("line {lineno}: bad checksum");
        }
        let addr = u16::from_be_bytes([bytes[1], bytes[2]]) as u32;
        let data = &bytes[4..4 + len];
        match bytes[3] {
            0x00 => records.push(Record {
                addr: base + addr,
                data: data.to_vec(),
            }),
            0x01 => {
                saw_eof = true;
                break;
            }
            0x02 if len == 2 => base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 4,
            0x04 if len == 2 => base = (u16::from_be_bytes([data[0], data[1]]) as u32) << 16,
            0x03 | 0x05 => {} // Start address: not needed for the FX2.
            t => bail!("line {lineno}: unsupported record type {t:#04x}"),
        }
    }
    if !saw_eof {
        bail!("no end-of-file record");
    }
    Ok(records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_data_and_eof() {
        let text = ":0300300002337A1E\n:00000001FF\n";
        let r = parse(text).unwrap();
        assert_eq!(
            r,
            vec![Record {
                addr: 0x0030,
                data: vec![0x02, 0x33, 0x7A]
            }]
        );
    }

    #[test]
    fn extended_linear_address() {
        let text = ":020000040001F9\n:0100100055 9A\n:00000001FF\n".replace(' ', "");
        let r = parse(&text).unwrap();
        assert_eq!(r[0].addr, 0x10010);
    }

    #[test]
    fn rejects_bad_checksum() {
        let err = parse(":0300300002337A1F\n:00000001FF\n").unwrap_err();
        assert!(err.to_string().contains("checksum"));
    }

    #[test]
    fn rejects_missing_eof() {
        assert!(parse(":0300300002337A1E\n").is_err());
    }

    #[test]
    fn rejects_junk() {
        assert!(parse("hello\n").is_err());
        assert!(parse(":03003000\n").is_err());
        assert!(parse(":0400300002337A1E\n:00000001FF\n").is_err());
    }

    #[test]
    fn parses_real_firmware_when_present() {
        // The OpenHantek6022 firmware, if the build knows where it is.
        let Some(path) = option_env!("OPENHANTEK_TUI_FIRMWARE") else {
            return;
        };
        let text = std::fs::read_to_string(path).unwrap();
        let r = parse(&text).unwrap();
        assert!(!r.is_empty());
        // FX2 internal RAM is 16 KiB (0x0000..0x4000).
        assert!(r.iter().all(|rec| rec.addr as usize + rec.data.len() <= 0x4000));
    }
}

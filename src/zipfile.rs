//! A minimal ZIP archive writer - just enough for the MP3+G `.zip` export
//! (a `.cdg` and its `.mp3` side by side, the way karaoke hosts and KJ
//! software import songs). Writes plain PKZIP 2.0 archives: deflate or
//! stored entries, no ZIP64 (both files are far below 4GB), no encryption.
//! Built on `flate2`/`crc32fast`, which were already in the dependency tree,
//! rather than pulling in a full `zip` crate for one writer.

use anyhow::{bail, Context, Result};
use flate2::write::DeflateEncoder;
use flate2::Compression;
use std::io::Write;
use std::path::Path;

const LOCAL_HEADER_SIG: u32 = 0x0403_4b50;
const CENTRAL_HEADER_SIG: u32 = 0x0201_4b50;
const END_OF_CENTRAL_SIG: u32 = 0x0605_4b50;
const METHOD_STORED: u16 = 0;
const METHOD_DEFLATE: u16 = 8;
/// "Version needed to extract": 2.0 (deflate).
const VERSION: u16 = 20;
/// General purpose flag bit 11: file names are UTF-8.
const FLAG_UTF8: u16 = 1 << 11;

/// Writes `entries` (name inside the archive, contents) to a new zip at
/// `path`, replacing any existing file. Each entry is deflated, or stored
/// as-is if deflating doesn't make it smaller (already-compressed MP3s).
pub fn write_zip(path: &Path, entries: &[(&str, &[u8])]) -> Result<()> {
    let bytes = build_zip(entries, dos_now())?;
    std::fs::write(path, bytes).with_context(|| format!("failed to write {}", path.display()))
}

fn build_zip(entries: &[(&str, &[u8])], (dos_time, dos_date): (u16, u16)) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in entries {
        if data.len() > u32::MAX as usize || out.len() > u32::MAX as usize {
            bail!("{name} is too large for a zip archive");
        }
        let crc = crc32fast::hash(data);
        let deflated = {
            let mut enc = DeflateEncoder::new(Vec::new(), Compression::default());
            enc.write_all(data)?;
            enc.finish()?
        };
        let (method, payload): (u16, &[u8]) = if deflated.len() < data.len() {
            (METHOD_DEFLATE, &deflated)
        } else {
            (METHOD_STORED, data)
        };
        let offset = out.len() as u32;
        let name_bytes = name.as_bytes();

        // Local file header.
        put32(&mut out, LOCAL_HEADER_SIG);
        put16(&mut out, VERSION);
        put16(&mut out, FLAG_UTF8);
        put16(&mut out, method);
        put16(&mut out, dos_time);
        put16(&mut out, dos_date);
        put32(&mut out, crc);
        put32(&mut out, payload.len() as u32);
        put32(&mut out, data.len() as u32);
        put16(&mut out, name_bytes.len() as u16);
        put16(&mut out, 0); // extra field length
        out.extend_from_slice(name_bytes);
        out.extend_from_slice(payload);

        // Central directory entry.
        put32(&mut central, CENTRAL_HEADER_SIG);
        put16(&mut central, VERSION); // version made by
        put16(&mut central, VERSION);
        put16(&mut central, FLAG_UTF8);
        put16(&mut central, method);
        put16(&mut central, dos_time);
        put16(&mut central, dos_date);
        put32(&mut central, crc);
        put32(&mut central, payload.len() as u32);
        put32(&mut central, data.len() as u32);
        put16(&mut central, name_bytes.len() as u16);
        put16(&mut central, 0); // extra
        put16(&mut central, 0); // comment
        put16(&mut central, 0); // disk number
        put16(&mut central, 0); // internal attributes
        put32(&mut central, 0); // external attributes
        put32(&mut central, offset);
        central.extend_from_slice(name_bytes);
    }
    let central_offset = out.len() as u32;
    out.extend_from_slice(&central);
    put32(&mut out, END_OF_CENTRAL_SIG);
    put16(&mut out, 0); // this disk
    put16(&mut out, 0); // disk with central directory
    put16(&mut out, entries.len() as u16);
    put16(&mut out, entries.len() as u16);
    put32(&mut out, central.len() as u32);
    put32(&mut out, central_offset);
    put16(&mut out, 0); // comment length
    Ok(out)
}

fn put16(out: &mut Vec<u8>, v: u16) {
    out.extend_from_slice(&v.to_le_bytes());
}

fn put32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_le_bytes());
}

/// The current time (UTC) as MS-DOS `(time, date)` fields.
fn dos_now() -> (u16, u16) {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    dos_from_unix(secs)
}

/// Unix seconds -> MS-DOS `(time, date)`, clamped to DOS's 1980 epoch.
fn dos_from_unix(secs: u64) -> (u16, u16) {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    if year < 1980 {
        return (0, (1 << 5) | 1); // 1980-01-01 00:00
    }
    let time = ((rem / 3600) << 11) | (((rem % 3600) / 60) << 5) | ((rem % 60) / 2);
    let date = (((year - 1980) as u64) << 9) | ((month as u64) << 5) | day as u64;
    (time as u16, date as u16)
}

#[cfg(test)]
mod tests {
    use super::*;
    use flate2::read::DeflateDecoder;
    use std::io::Read;

    fn u16_at(b: &[u8], i: usize) -> u16 {
        u16::from_le_bytes([b[i], b[i + 1]])
    }
    fn u32_at(b: &[u8], i: usize) -> u32 {
        u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
    }

    /// Reads every entry back out by walking the central directory, the
    /// way real unzip tools do - checking names, CRCs and sizes.
    fn read_back(zip: &[u8]) -> Vec<(String, Vec<u8>)> {
        let eocd = zip.len() - 22;
        assert_eq!(u32_at(zip, eocd), END_OF_CENTRAL_SIG);
        let count = u16_at(zip, eocd + 10) as usize;
        let mut cd = u32_at(zip, eocd + 16) as usize;
        let mut out = Vec::new();
        for _ in 0..count {
            assert_eq!(u32_at(zip, cd), CENTRAL_HEADER_SIG);
            let method = u16_at(zip, cd + 10);
            let crc = u32_at(zip, cd + 16);
            let csize = u32_at(zip, cd + 20) as usize;
            let usize_ = u32_at(zip, cd + 24) as usize;
            let nlen = u16_at(zip, cd + 28) as usize;
            let local = u32_at(zip, cd + 42) as usize;
            let name = String::from_utf8(zip[cd + 46..cd + 46 + nlen].to_vec()).unwrap();
            assert_eq!(u32_at(zip, local), LOCAL_HEADER_SIG);
            let lnlen = u16_at(zip, local + 26) as usize;
            let start = local + 30 + lnlen;
            let payload = &zip[start..start + csize];
            let data = match method {
                METHOD_STORED => payload.to_vec(),
                METHOD_DEFLATE => {
                    let mut d = Vec::new();
                    DeflateDecoder::new(payload).read_to_end(&mut d).unwrap();
                    d
                }
                m => panic!("unexpected method {m}"),
            };
            assert_eq!(data.len(), usize_);
            assert_eq!(crc32fast::hash(&data), crc);
            out.push((name, data));
            cd += 46 + nlen;
        }
        out
    }

    #[test]
    fn round_trips_compressible_and_incompressible_entries() {
        let cdg = vec![0u8; 24 * 3000]; // mostly-empty packets: compresses
        let mp3: Vec<u8> = (0..5000u32)
            .map(|i| (i.wrapping_mul(2_654_435_761) >> 13) as u8)
            .collect();
        let zip = build_zip(&[("Song.cdg", &cdg), ("Song.mp3", &mp3)], (0, 33)).unwrap();
        let entries = read_back(&zip);
        assert_eq!(entries[0], ("Song.cdg".to_string(), cdg));
        assert_eq!(entries[1], ("Song.mp3".to_string(), mp3));
        assert!(zip.len() < 24 * 3000, "cdg should have been deflated");
    }

    #[test]
    fn utf8_names_survive() {
        let zip = build_zip(&[("Café – Señor.cdg", b"x")], (0, 33)).unwrap();
        assert_eq!(read_back(&zip)[0].0, "Café – Señor.cdg");
    }

    #[test]
    fn dos_timestamps() {
        // 2026-11-13 14:30:58 UTC
        let (time, date) = dos_from_unix(1_794_580_258);
        assert_eq!(date >> 9, 2026 - 1980);
        assert_eq!((date >> 5) & 0x0F, 11);
        assert_eq!(date & 0x1F, 13);
        assert_eq!(time >> 11, 14);
        assert_eq!((time >> 5) & 0x3F, 30);
        assert_eq!((time & 0x1F) * 2, 58);
        // Before 1980 clamps to the DOS epoch.
        assert_eq!(dos_from_unix(0), (0, (1 << 5) | 1));
    }
}

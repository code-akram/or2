//! A read-only zip reader, just enough for the jar/aar files in the Gradle cache: the central
//! directory, stored and deflated entries, no zip64, no encryption.

use std::io::Read;

use flate2::read::DeflateDecoder;

use crate::{Result, fail};

pub struct Entry {
    pub name: String,
    pub size: u64,
    method: u16,
    compressed: u64,
    local_offset: u64,
}

pub struct Archive<'a> {
    data: &'a [u8],
    pub entries: Vec<Entry>,
}

fn u16_at(data: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(data.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(data: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(data.get(at..at + 4)?.try_into().ok()?))
}

impl<'a> Archive<'a> {
    pub fn open(data: &'a [u8]) -> Result<Self> {
        const EOCD: u32 = 0x0605_4b50;
        const CENTRAL: u32 = 0x0201_4b50;
        let malformed = || "malformed zip archive".to_string();
        let search_from = data.len().saturating_sub(22 + 65_535);
        let eocd = (search_from..=data.len().saturating_sub(22))
            .rev()
            .find(|&at| u32_at(data, at) == Some(EOCD))
            .ok_or_else(malformed)?;
        let count = u16_at(data, eocd + 10).ok_or_else(malformed)? as usize;
        let mut at = u32_at(data, eocd + 16).ok_or_else(malformed)? as usize;
        let mut entries = Vec::with_capacity(count);
        for _ in 0..count {
            if u32_at(data, at) != Some(CENTRAL) {
                return Err(malformed());
            }
            let field16 = |offset| u16_at(data, at + offset).ok_or_else(malformed);
            let field32 = |offset| u32_at(data, at + offset).ok_or_else(malformed);
            let name_len = field16(28)? as usize;
            let extra_len = field16(30)? as usize;
            let comment_len = field16(32)? as usize;
            let name = data
                .get(at + 46..at + 46 + name_len)
                .ok_or_else(malformed)?;
            entries.push(Entry {
                name: String::from_utf8_lossy(name).into_owned(),
                size: u64::from(field32(24)?),
                method: field16(10)?,
                compressed: u64::from(field32(20)?),
                local_offset: u64::from(field32(42)?),
            });
            at += 46 + name_len + extra_len + comment_len;
        }
        Ok(Self { data, entries })
    }

    pub fn read(&self, entry: &Entry) -> Result<Vec<u8>> {
        const LOCAL: u32 = 0x0403_4b50;
        let malformed = || format!("malformed zip entry {}", entry.name);
        let at = entry.local_offset as usize;
        if u32_at(self.data, at) != Some(LOCAL) {
            return Err(malformed());
        }
        let name_len = u16_at(self.data, at + 26).ok_or_else(malformed)? as usize;
        let extra_len = u16_at(self.data, at + 28).ok_or_else(malformed)? as usize;
        let start = at + 30 + name_len + extra_len;
        let raw = self
            .data
            .get(start..start + entry.compressed as usize)
            .ok_or_else(malformed)?;
        match entry.method {
            0 => Ok(raw.to_vec()),
            8 => {
                let mut out = Vec::with_capacity(entry.size as usize);
                DeflateDecoder::new(raw)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("{}: {e}", entry.name))?;
                Ok(out)
            }
            other => fail(format!("{}: unsupported zip method {other}", entry.name)),
        }
    }
}

#[cfg(test)]
pub fn stored_archive(files: &[(&str, &str)]) -> Vec<u8> {
    // Hand-assembled zip with stored entries: local headers, central directory, end record.
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, body) in files {
        let offset = out.len() as u32;
        let (len, name_len) = (body.len() as u32, name.len() as u16);
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(0u32.to_le_bytes()); // crc (not checked)
        out.extend(len.to_le_bytes());
        out.extend(len.to_le_bytes());
        out.extend(name_len.to_le_bytes());
        out.extend(0u16.to_le_bytes());
        out.extend(name.as_bytes());
        out.extend(body.as_bytes());

        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(0u32.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend(len.to_le_bytes());
        central.extend(name_len.to_le_bytes());
        central.extend([0u8; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let central_offset = out.len() as u32;
    let central_len = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend((files.len() as u16).to_le_bytes());
    out.extend((files.len() as u16).to_le_bytes());
    out.extend(central_len.to_le_bytes());
    out.extend(central_offset.to_le_bytes());
    out.extend([0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_stored_entries_by_name() {
        let data = stored_archive(&[("META-INF/LICENSE", "terms\n"), ("a/b.class", "x")]);
        let archive = Archive::open(&data).unwrap();
        let names: Vec<_> = archive.entries.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["META-INF/LICENSE", "a/b.class"]);
        assert_eq!(archive.read(&archive.entries[0]).unwrap(), b"terms\n");
        assert_eq!(archive.entries[1].size, 1);
    }

    #[test]
    fn reads_deflated_entries() {
        use flate2::{Compression, write::DeflateEncoder};
        use std::io::Write;
        let mut encoder = DeflateEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(b"licence licence licence").unwrap();
        let packed = encoder.finish().unwrap();
        // Reuse the stored layout, then patch the method and sizes to describe the packed bytes.
        let mut data = stored_archive(&[(
            "LICENSE",
            std::str::from_utf8(&vec![b'z'; packed.len()]).unwrap(),
        )]);
        let name_len = "LICENSE".len();
        data[8] = 8; // local method
        data[30 + name_len..30 + name_len + packed.len()].copy_from_slice(&packed);
        let central = 30 + name_len + packed.len();
        data[central + 10] = 8; // central method
        let archive = Archive::open(&data).unwrap();
        assert_eq!(
            archive.read(&archive.entries[0]).unwrap(),
            b"licence licence licence"
        );
    }

    #[test]
    fn garbage_is_rejected() {
        assert!(Archive::open(b"not a zip").is_err());
    }
}

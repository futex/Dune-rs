//! `DUNE.DAT` archive.
//!
//! Layout (reversed from the file):
//! ```text
//! 0x0000  u16      unknown (0x0A3D)
//! 0x0002  entry[]  25 bytes each, until an entry with an empty name
//!         char[16] name (NUL padded, DOS path, may contain '\')
//!         u32      size
//!         u32      offset (absolute)
//!         u8       flags (always 0)
//! 0x10000 file data
//! ```

use std::collections::HashMap;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;
use std::sync::Mutex;

pub const HEADER_SIZE: usize = 0x10000;
const ENTRY_SIZE: usize = 25;

#[derive(Debug, Clone)]
pub struct DatEntry {
    pub name: String,
    pub size: u32,
    pub offset: u32,
    pub flags: u8,
}

pub struct DatArchive {
    file: Mutex<File>,
    entries: Vec<DatEntry>,
    index: HashMap<String, usize>,
}

impl DatArchive {
    pub fn open(path: impl AsRef<Path>) -> io::Result<Self> {
        let mut file = File::open(path)?;
        let mut hdr = vec![0u8; HEADER_SIZE];
        file.read_exact(&mut hdr)?;
        let entries = parse_table(&hdr);
        let mut index = HashMap::new();
        for (i, e) in entries.iter().enumerate() {
            // Some names are duplicated (e.g. VG01.HSQ); the first one wins, like a linear search would.
            index.entry(e.name.to_ascii_uppercase()).or_insert(i);
        }
        Ok(Self { file: Mutex::new(file), entries, index })
    }

    pub fn entries(&self) -> &[DatEntry] {
        &self.entries
    }

    pub fn find(&self, name: &str) -> Option<&DatEntry> {
        self.index
            .get(&name.to_ascii_uppercase().replace('/', "\\"))
            .map(|&i| &self.entries[i])
    }

    /// Raw bytes of an entry, as stored.
    pub fn read_entry(&self, e: &DatEntry) -> io::Result<Vec<u8>> {
        let mut f = self.file.lock().unwrap();
        f.seek(SeekFrom::Start(e.offset as u64))?;
        let mut buf = vec![0u8; e.size as usize];
        f.read_exact(&mut buf)?;
        Ok(buf)
    }

    /// Raw bytes by name.
    pub fn read_raw(&self, name: &str) -> io::Result<Vec<u8>> {
        let e = self
            .find(name)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, format!("{name} not in DUNE.DAT")))?
            .clone();
        self.read_entry(&e)
    }

    /// Bytes by name, transparently HSQ-decompressed when the data carries a valid HSQ header.
    pub fn read(&self, name: &str) -> io::Result<Vec<u8>> {
        let raw = self.read_raw(name)?;
        if crate::hsq::is_hsq(&raw) {
            crate::hsq::decompress(&raw).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("{name}: {e}")))
        } else {
            Ok(raw)
        }
    }
}

fn parse_table(hdr: &[u8]) -> Vec<DatEntry> {
    let mut out = Vec::new();
    let mut p = 2;
    while p + ENTRY_SIZE <= hdr.len() {
        let raw_name = &hdr[p..p + 16];
        let len = raw_name.iter().position(|&b| b == 0).unwrap_or(16);
        if len == 0 {
            break;
        }
        let name: String = raw_name[..len].iter().map(|&b| b as char).collect();
        let size = u32::from_le_bytes(hdr[p + 16..p + 20].try_into().unwrap());
        let offset = u32::from_le_bytes(hdr[p + 20..p + 24].try_into().unwrap());
        out.push(DatEntry { name, size, offset, flags: hdr[p + 24] });
        p += ENTRY_SIZE;
    }
    out
}

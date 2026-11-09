//! Access to the original game data, with caching of decoded resources.

use dune_formats::{
    hnm::Hnm,
    sprite::SpriteSheet,
    text::{Font, StringTable},
    DatArchive,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

pub struct Assets {
    pub dat: DatArchive,
    sheets: HashMap<String, Option<Rc<SpriteSheet>>>,
    pub font: Font,
    /// Small 7-pixel font.
    pub font2: Font,
    pub lang: u8,
    /// DNCDPRG.EXE (found next to DUNE.DAT), for the game's static tables.
    pub exe: Option<Vec<u8>>,
    /// Folder holding DUNE.DAT (save games go there too).
    pub dat_dir: PathBuf,
}

/// File offset of the EXE's data segment (ds:0000).
pub const EXE_DS: usize = 0xF6B0;

impl Assets {
    pub fn open(path: &Path, lang: u8) -> std::io::Result<Assets> {
        let dat = DatArchive::open(path)?;
        let font = Font::parse(&dat.read("DNCHAR.BIN")?).expect("DNCHAR.BIN");
        let font2 = Font::parse_small(&dat.read("DNCHAR.BIN")?).expect("DNCHAR.BIN small font");
        let exe = path.parent().map(|d| d.join("DNCDPRG.EXE")).and_then(|p| std::fs::read(p).ok());
        if exe.is_none() {
            eprintln!("note: DNCDPRG.EXE not found next to DUNE.DAT; some tables will be missing");
        }
        let dat_dir = path.parent().map(|p| p.to_path_buf()).unwrap_or_default();
        Ok(Assets { dat, sheets: HashMap::new(), font, font2, lang, exe, dat_dir })
    }

    /// Looks for DUNE.DAT in the usual places.
    pub fn locate(explicit: Option<&str>) -> Option<PathBuf> {
        let mut cands: Vec<PathBuf> = Vec::new();
        if let Some(p) = explicit {
            cands.push(p.into());
        }
        if let Ok(p) = std::env::var("DUNE_DAT") {
            cands.push(p.into());
        }
        // Default location: a `datas` folder next to the executable.
        if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|d| d.to_path_buf())) {
            cands.push(dir.join("datas").join("DUNE.DAT"));
        }
        for base in [".", "..", "../..", "../work/iso", "../../work/iso", "work/iso"] {
            cands.push(Path::new(base).join("DUNE.DAT"));
        }
        cands.into_iter().find(|p| p.is_file())
    }

    /// Reads `n` little-endian words at `ds:off` in the EXE.
    pub fn exe_words(&self, off: usize, n: usize) -> Option<Vec<u16>> {
        let e = self.exe.as_ref()?;
        (0..n).map(|i| e.get(EXE_DS + off + 2 * i..EXE_DS + off + 2 * i + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))).collect()
    }

    /// Reads `n` bytes at `ds:off` in the EXE.
    pub fn exe_bytes(&self, off: usize, n: usize) -> Option<&[u8]> {
        self.exe.as_ref()?.get(EXE_DS + off..EXE_DS + off + n)
    }

    /// Name of resource `i` in the EXE's resource table (ds:31FF: u16 ptr -> u16 word + name).
    pub fn resource_name(&self, i: usize) -> Option<String> {
        let p = *self.exe_words(0x31FF + 2 * i, 1)?.first()? as usize;
        let b = self.exe_bytes(p + 2, 16)?;
        let n: String = b.iter().take_while(|&&c| c != 0).map(|&c| c as char).collect();
        (!n.is_empty()).then_some(n)
    }

    pub fn sheet(&mut self, name: &str) -> Option<Rc<SpriteSheet>> {
        if let Some(s) = self.sheets.get(name) {
            return s.clone();
        }
        let s = self.dat.read(name).ok().and_then(|d| SpriteSheet::parse(&d)).map(Rc::new);
        self.sheets.insert(name.to_string(), s.clone());
        s
    }

    pub fn hnm(&self, name: &str) -> Option<Hnm> {
        self.dat.read(name).ok().and_then(Hnm::parse)
    }

    pub fn strings(&self, name: &str) -> Option<StringTable> {
        self.dat.read(name).ok().and_then(|d| StringTable::parse(&d))
    }

    pub fn phrases(&self, bank: u8) -> Option<StringTable> {
        self.strings(&format!("PHRASE{}{}.HSQ", self.lang, bank))
    }

    pub fn commands(&self) -> Option<StringTable> {
        self.strings(&format!("COMMAND{}.HSQ", self.lang))
    }

    pub fn names_with_ext(&self, ext: &str) -> Vec<String> {
        let mut seen = std::collections::HashSet::new();
        self.dat
            .entries()
            .iter()
            .filter(|e| e.name.ends_with(ext) && seen.insert(e.name.clone()))
            .map(|e| e.name.clone())
            .collect()
    }
}

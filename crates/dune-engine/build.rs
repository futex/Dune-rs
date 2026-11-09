//! Copies the game data folder (`<workspace>/datas`: DUNE.DAT, DNCDPRG.EXE) next to the
//! built binary (`target/<profile>/datas`), where `dune` looks for it at start-up.
//! Files are hard-linked when possible (no 400 MB copy) and only refreshed when they change;
//! other files already there (save games) are left alone.

use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let src = manifest.join("../../datas");
    println!("cargo:rerun-if-changed={}", src.display());
    if !src.is_dir() {
        println!("cargo:warning=no datas/ folder at the workspace root: put DUNE.DAT and DNCDPRG.EXE there");
        return;
    }
    // OUT_DIR = target/<profile>/build/<pkg>-<hash>/out  ->  target/<profile>
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let Some(profile_dir) = out.ancestors().nth(3) else { return };
    let dst = profile_dir.join("datas");
    if let Err(e) = sync_dir(&src, &dst) {
        println!("cargo:warning=could not copy datas/ to {}: {e}", dst.display());
    }
}

fn sync_dir(src: &Path, dst: &Path) -> std::io::Result<()> {
    fs::create_dir_all(dst)?;
    for entry in fs::read_dir(src)? {
        let entry = entry?;
        let from = entry.path();
        println!("cargo:rerun-if-changed={}", from.display());
        let to = dst.join(entry.file_name());
        if from.is_dir() {
            sync_dir(&from, &to)?;
            continue;
        }
        let meta = entry.metadata()?;
        let up_to_date = fs::metadata(&to)
            .map(|m| m.len() == meta.len() && m.modified().ok() >= meta.modified().ok())
            .unwrap_or(false);
        if up_to_date {
            continue;
        }
        let _ = fs::remove_file(&to);
        if fs::hard_link(&from, &to).is_err() {
            fs::copy(&from, &to)?;
        }
    }
    Ok(())
}

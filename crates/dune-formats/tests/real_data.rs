//! Checks against the user's own DUNE.DAT. Skipped when DUNE_DAT is not set.

use dune_formats::{hnm::Hnm, hsq, sprite::SpriteSheet, text::StringTable, voc::Voc, DatArchive};

fn dat() -> Option<DatArchive> {
    let p = std::env::var("DUNE_DAT").ok()?;
    Some(DatArchive::open(p).expect("open DUNE.DAT"))
}

#[test]
fn every_hsq_decompresses_to_declared_size() {
    let Some(d) = dat() else { return };
    for e in d.entries() {
        let raw = d.read_entry(e).unwrap();
        if hsq::header_ok(&raw) {
            assert_eq!(hsq::decompress(&raw).unwrap().len(), hsq::unpacked_size(&raw), "{}", e.name);
        }
    }
}

#[test]
fn known_resources_parse() {
    let Some(d) = dat() else { return };
    let icones = SpriteSheet::parse(&d.read("ICONES.HSQ").unwrap()).unwrap();
    assert_eq!(icones.sprites.len(), 77);
    assert_eq!((icones.sprites[0].width, icones.sprites[0].height), (92, 48));
    let h = Hnm::parse(d.read("CRYO.HNM").unwrap()).unwrap();
    assert_eq!(h.frame_count(), 101);
    let t = StringTable::parse(&d.read("PHRASE11.HSQ").unwrap()).unwrap();
    assert_eq!(t.get(0), b"I am the Duke Leto Atreides, your father.");
    let v = Voc::parse(&d.read("PA\\PA190I.VOC").unwrap()).unwrap();
    assert_eq!(v.sample_rate, 22222);
    assert!(v.lipsync.is_some());
}

#[test]
fn every_hnm_parses() {
    let Some(d) = dat() else { return };
    for e in d.entries().iter().filter(|e| e.name.ends_with(".HNM")) {
        let h = Hnm::parse(d.read(&e.name).unwrap()).unwrap_or_else(|| panic!("{}", e.name));
        for i in 0..h.frame_count() {
            let _ = h.frame(i);
        }
    }
}

#[test]
fn dialogue_and_conditions() {
    use dune_formats::dialogue::{Condit, Dialogue};
    let Some(d) = dat() else { return };
    let dl = Dialogue::parse(d.read("DIALOGUE.HSQ").unwrap()).unwrap();
    let c = Condit::parse(d.read("CONDIT.HSQ").unwrap()).unwrap();
    // Condition 2 = (b[2A]==1) AND (((w[10]|w[12]) & 0x10) == 0)  (docs/re/gameflow.md)
    let mut m = vec![0u8; 0x100];
    m[0x2A] = 1;
    assert!(c.eval(2, &m));
    m[0x10] = 0x10;
    assert!(!c.eval(2, &m));
    assert_eq!(dl.first_phrase(1), 50);
}

#[test]
fn music_lengths_match_driver() {
    use dune_formats::herad::{Herad, OplOut, IRQ_HZ};
    struct Null;
    impl OplOut for Null {
        fn write(&mut self, _: u8, _: u8) {}
    }
    let Some(d) = dat() else { return };
    for (song, secs) in [("WATER.HSQ", 47.9), ("CRYOMUS.HSQ", 29.0), ("SIETCHM.HSQ", 89.3)] {
        let mut h = Herad::new(d.read(song).unwrap(), 1, &mut Null).unwrap();
        let mut irqs = 0u64;
        while h.playing && irqs < 200_000 {
            h.interrupt(&mut Null);
            irqs += 1;
        }
        let t = irqs as f64 / IRQ_HZ;
        assert!((t - secs).abs() < 0.2, "{song}: {t:.1}s vs {secs}");
    }
}

#[test]
fn every_sal_room_parses() {
    use dune_formats::sal::Sal;
    let Some(d) = dat() else { return };
    for (f, n) in [("PALACE.SAL", 15), ("SIET.SAL", 14), ("VILG.SAL", 11), ("HARK.SAL", 8)] {
        let s = Sal::parse(&d.read(f).unwrap()).unwrap();
        assert_eq!(s.rooms.len(), n, "{f}");
        for r in &s.rooms {
            assert_eq!(r.markers().len(), r.nslots as usize, "{f}");
        }
    }
}

#[test]
fn globe_marker_and_pick() {
    let Some(d) = dat() else { return };
    let mut g = dune_formats::globe::Globe::new(&d.read("TABLAT.BIN").unwrap(), d.read("GLOBDATA.HSQ").unwrap()).unwrap();
    g.set_view(0x1964, -4);
    // docs/re/globe.md §4.1: the Arrakeen marker is at (160,103) and picks back 0x1915, -3.
    assert_eq!(g.marker(0x1915, -4), Some((160, 103)));
    assert_eq!(g.pick(160, 103), Some((0x1915, -3)));
    assert_eq!(g.pick(10, 10), None);
}

#[test]
fn planet_tables_and_locations() {
    let Some(d) = dat() else { return };
    use dune_formats::planet;
    let mut mem = vec![0u8; 0x10000];
    planet::init_tables(&mut mem, &d.read("TABLAT.BIN").unwrap());
    assert_eq!(planet::half(&mem, 0), 199);
    // Row 0 starts at the map base; Carthag (Atreides) snapped longitude stays 0x1915.
    assert_eq!(planet::snap_lon(&mem, 0x1915, -4), 0x1915);
    assert_eq!(planet::snap_lon(&mem, 0x3A98, -62), 0x3ACF);
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf29ce484222325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100000001b3))
}

/// The map renderers were checked pixel-for-pixel against the original code (DNVGA k=29 /
/// k=22 run under an x86 emulator); these hashes pin that output.
#[test]
fn planet_renderers_regression() {
    let Some(d) = dat() else { return };
    use dune_formats::planet;
    let map = d.read("MAP.HSQ").unwrap();
    let drv = planet::DriverTables::new(d.read("DNVGA.HSQ").unwrap());
    let mut mem = vec![0u8; 0x10000];
    planet::init_tables(&mut mem, &d.read("TABLAT.BIN").unwrap());
    mem[0x197C..0x197E].copy_from_slice(&0x1915u16.to_le_bytes());
    mem[0x197E..0x1980].copy_from_slice(&0xFFFCu16.to_le_bytes());
    mem[0x46EB] = 0x80;
    let mut fb = vec![0u8; 64000];
    planet::render_map(&mut mem, &map, &mut fb, &drv);
    let h1 = fnv(&fb);
    mem[0x46EB] = 1;
    for (k, v) in [81u16, 45, 241, 134].iter().enumerate() {
        mem[0x46E3 + 2 * k..0x46E5 + 2 * k].copy_from_slice(&v.to_le_bytes());
    }
    let mut fb2 = vec![0u8; 64000];
    planet::render_flight(&mut mem, &map, &mut fb2, &drv);
    let h2 = fnv(&fb2);
    println!("{h1:#x} {h2:#x}");
    assert_eq!((h1, h2), (EXPECTED_MAP, EXPECTED_FLIGHT));
}
const EXPECTED_MAP: u64 = 0x61f0f3bf48e6f10c;
const EXPECTED_FLIGHT: u64 = 0xd1c3f183717e0f69;

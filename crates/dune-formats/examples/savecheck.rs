use dune_formats::save::*;
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dat = dune_formats::DatArchive::open(&a[1]).unwrap();
    let f = std::fs::read(&a[2]).unwrap();
    let s = SaveGame::parse(&f).expect("parse");
    println!("time {} map_bits {} cs {} aa76 {} low {}", s.time, s.map_bits.len(), s.cs_vars.len(), s.ds_aa76.len(), s.ds_low.len());
    let mut map = dat.read("MAP.HSQ").unwrap();
    map.resize(MAP_SIZE, 0);
    let fresh = pack_map(&map);
    let same = fresh.iter().zip(&s.map_bits).filter(|(a, b)| a == b).count();
    println!("map bits equal to fresh MAP.HSQ: {same}/{}", fresh.len());
    let dia = dat.read("DIALOGUE.HSQ").unwrap();
    let dsame = dia.iter().zip(&s.ds_aa76).filter(|(a, b)| (*a & 0x7F) == (*b & 0x7F)).count();
    println!("dialogue bytes equal (ignoring said bit): {dsame}/{}", dia.len());
    println!("ds:4 {:#06x} ds:6 {:#06x} ds:2A {}", u16::from_le_bytes([s.ds_low[4], s.ds_low[5]]), u16::from_le_bytes([s.ds_low[6], s.ds_low[7]]), s.ds_low[0x2A]);
    let again = SaveGame::parse(&s.to_bytes()).unwrap();
    println!("roundtrip ok: {}", again.to_bytes() == s.to_bytes() && again.ds_low == s.ds_low && again.map_bits == s.map_bits);
}

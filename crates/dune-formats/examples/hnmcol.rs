fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dat = dune_formats::DatArchive::open(&a[1]).unwrap();
    let h = dune_formats::hnm::Hnm::parse(dat.read(&a[2]).unwrap()).unwrap();
    let mut p = dune_formats::hnm::HnmPlayer::new(h);
    p.step();
    let mut hist = std::collections::BTreeMap::new();
    for y in 0..40 { for x in 0..320 { *hist.entry(p.fb[y*320+x]).or_insert(0) += 1; } }
    println!("top rows: {:?}", hist);
    let mut all = std::collections::BTreeSet::new();
    for v in &p.fb[..320*152] { all.insert(*v); }
    println!("min {:?} max {:?}", all.iter().next(), all.iter().last());
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dat = dune_formats::DatArchive::open(&a[1]).unwrap();
    let mut g = dune_formats::globe::Globe::new(&dat.read("TABLAT.BIN").unwrap(), dat.read("GLOBDATA.HSQ").unwrap()).unwrap();
    g.set_view(0x1964, -4);
    let map = dat.read("MAP.HSQ").unwrap();
    let mut fb = vec![0u8; 64000];
    g.render(&mut fb, &map);
    std::fs::write(&a[2], &fb).unwrap();
    g.rotate(40);
    let mut fb2 = vec![0u8; 64000];
    g.render(&mut fb2, &map);
    std::fs::write(format!("{}.r40", a[2]), &fb2).unwrap();
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dat = dune_formats::DatArchive::open(&a[1]).unwrap();
    for n in ["PA\\PA001I.VOC", "PA\\PA190I.VOC", "PB\\PB005I.VOC", "PJ\\PJ001I.VOC", "PE\\PE013I.VOC"] {
        let v = dune_formats::voc::Voc::parse(&dat.read(n).unwrap()).unwrap();
        let l = v.lipsync.unwrap();
        let secs = v.pcm.len() as f32 / v.sample_rate as f32;
        let hdr = u16::from_le_bytes([l[0], l[1]]);
        println!("{n}: {secs:.2}s len {} hdr {hdr} -> {:.2}/s, last {:?}", l.len(), (l.len() - 3) as f32 / secs, &l[l.len() - 3..]);
    }
}

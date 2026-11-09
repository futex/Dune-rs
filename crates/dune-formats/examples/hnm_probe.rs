use dune_formats::hsq;
fn main() {
    let path = std::env::args().nth(1).unwrap();
    let d = std::fs::read(&path).unwrap();
    let hs = u16::from_le_bytes([d[0], d[1]]) as usize;
    let mut pal = dune_formats::palette::Palette::black();
    let mut p = 2 + pal.apply_chunks(&d[2..hs]);
    while p < hs && d[p] == 0xFF { p += 1 }
    let n = (hs - p) / 4;
    let offs: Vec<usize> = (0..n).map(|i| u32::from_le_bytes(d[p+4*i..p+4*i+4].try_into().unwrap()) as usize + hs).collect();
    for (fi, &o) in offs.iter().enumerate().take(std::env::args().nth(2).map(|s| s.parse().unwrap()).unwrap_or(5)) {
        let fs = u16::from_le_bytes([d[o], d[o+1]]) as usize;
        let mut q = o + 2;
        let end = o + fs;
        let mut desc = String::new();
        while q + 4 <= end {
            let t = &d[q..q+2];
            if t[0].is_ascii_lowercase() && t[1].is_ascii_lowercase() {
                let cs = u16::from_le_bytes([d[q+2], d[q+3]]) as usize;
                desc += &format!("[{}{} {}] ", t[0] as char, t[1] as char, cs);
                q += cs;
                continue;
            }
            let wf = u16::from_le_bytes([d[q], d[q+1]]);
            let (w, f, h, m) = (wf & 0x1ff, wf >> 9, d[q+2], d[q+3]);
            q += 4;
            desc += &format!("w{w} h{h} f{f:#x} m{m:#x} rest={} ", end - q);
            let body = &d[q..end];
            if hsq::header_ok(body) {
                let u = hsq::decompress(body).unwrap();
                desc += &format!("HSQ->{} (w*h={}) head={:02x?}", u.len(), w as usize*h as usize, &u[..u.len().min(12)]);
            } else {
                desc += &format!("raw head={:02x?}", &body[..body.len().min(12)]);
            }
            break;
        }
        println!("#{fi} size {fs}: {desc}");
    }
}

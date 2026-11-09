//! Compares the planet renderers with reference dumps made by running the original code
//! (scratchpad mapref/gen.py). Usage: mapcmp DUNE.DAT DNCDPRG.EXE REFDIR
use dune_formats::{planet, DatArchive};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dat = DatArchive::open(&a[1]).unwrap();
    let exe = std::fs::read(&a[2]).unwrap();
    let dir = std::path::Path::new(&a[3]);
    let map = dat.read("MAP.HSQ").unwrap();
    let drv = planet::DriverTables::new(dat.read("DNVGA.HSQ").unwrap());
    let cases = std::fs::read_to_string(dir.join("cases.txt")).unwrap();
    let mut bad = 0;
    for l in cases.lines() {
        let f: Vec<&str> = l.split_whitespace().collect();
        let (kind, i, lon, row): (&str, usize, i32, i32) = (f[0], f[1].parse().unwrap(), f[2].parse().unwrap(), f[3].parse().unwrap());
        let mut mem = exe[0xF6B0..].to_vec();
        mem.resize(0x10000, 0);
        planet::init_tables(&mut mem, &dat.read("TABLAT.BIN").unwrap());
        mem[0x197C..0x197E].copy_from_slice(&(lon as u16).to_le_bytes());
        mem[0x197E..0x1980].copy_from_slice(&(row as u16).to_le_bytes());
        let mut fb = vec![0u8; 64000];
        if kind == "m" {
            mem[0x46EB] = 0x80;
            planet::render_map(&mut mem, &map, &mut fb, &drv);
        } else {
            mem[0x46EB] = 1;
            for (k, v) in [81u16, 45, 241, 134].iter().enumerate() {
                mem[0x46E3 + 2 * k..0x46E5 + 2 * k].copy_from_slice(&v.to_le_bytes());
            }
            planet::render_flight(&mut mem, &map, &mut fb, &drv);
        }
        let r = std::fs::read(dir.join(format!("{kind}{i}.bin"))).unwrap();
        let diff = (0..64000).filter(|&p| fb[p] != r[p]).count();
        let tdiff = (0..792).filter(|&p| mem[0x4948 + p] != r[64000 + p]).count();
        if diff > 0 {
            let p = (0..64000).find(|&p| fb[p] != r[p]).unwrap();
            println!("{kind}{i}: first diff at ({},{}) got {:02x} want {:02x}", p % 320, p / 320, fb[p], r[p]);
        }
        println!("{kind}{i} lon={lon:04x} row={row}: {diff} pixel diffs, {tdiff} TABLAT diffs");
        bad += diff + tdiff;
    }
    // Coordinate conversions.
    let pts = std::fs::read_to_string(dir.join("pts.txt")).unwrap();
    let mut mem = exe[0xF6B0..].to_vec();
    mem.resize(0x10000, 0);
    planet::init_tables(&mut mem, &dat.read("TABLAT.BIN").unwrap());
    let mut cur = 0;
    for l in pts.lines() {
        let v: Vec<i64> = l.split_whitespace().skip(1).map(|x| x.parse().unwrap()).collect();
        if v[0] != cur {
            cur = v[0];
            mem[0x46EB] = v[0] as u8;
            mem[0x197C..0x197E].copy_from_slice(&0x1915u16.to_le_bytes());
            mem[0x197E..0x1980].copy_from_slice(&0xFFFCu16.to_le_bytes());
            for (k, val) in [81u16, 45, 241, 134].iter().enumerate() {
                mem[0x46E3 + 2 * k..0x46E5 + 2 * k].copy_from_slice(&val.to_le_bytes());
            }
            let mut fb = vec![0u8; 64000];
            if cur == 0x80 { planet::render_map(&mut mem, &map, &mut fb, &drv) } else { planet::render_flight(&mut mem, &map, &mut fb, &drv) }
        }
        let (lon, row) = (v[1] as u16, v[2] as i16);
        let (x, y) = planet::map_to_screen(&mem, lon, row);
        let (di, _) = planet::cell_ptr(&mem, lon, row);
        let s = planet::snap_lon(&mem, lon, row);
        let mut ok = (x as u16, y as u16, di as u16) == (v[3] as u16, v[4] as u16, v[5] as u16) && (v[6] < 0 || s == v[6] as u16);
        if v.len() > 7 {
            let (ql, qr) = planet::screen_to_map(&mem, v[7] as i32, v[8] as i32);
            ok &= (ql, qr as u16) == (v[9] as u16, v[10] as u16);
        }
        if !ok { println!("pt mismatch {l}: got {x} {y} {di} {s}"); bad += 1; }
    }
    println!("total {bad}");
}

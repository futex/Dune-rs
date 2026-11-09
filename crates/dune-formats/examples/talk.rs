use dune_formats::{dialogue::*, text::StringTable, DatArchive};
fn main() {
    let a: Vec<String> = std::env::args().collect();
    let dat = DatArchive::open(&a[1]).unwrap();
    let exe = std::fs::read(&a[2]).unwrap();
    let mut mem = exe[0xF6B0..].to_vec();
    mem[2] = 2; mem[3] = 0; // game clock start
    let d = Dialogue::parse(dat.read("DIALOGUE.HSQ").unwrap()).unwrap();
    let c = Condit::parse(dat.read("CONDIT.HSQ").unwrap()).unwrap();
    let mut z = vec![0u8; 0x100]; z[0x2A] = 1;
    println!("cond2 b2A=1 -> {}", c.eval(2, &z));
    z[0x10] = 0x10;
    println!("cond2 w10=0x10 -> {}", c.eval(2, &z));
    let p1 = StringTable::parse(&dat.read("PHRASE11.HSQ").unwrap()).unwrap();
    let p2 = StringTable::parse(&dat.read("PHRASE12.HSQ").unwrap()).unwrap();
    println!("ds:2A = {}  w10={:#x} w12={:#x}", mem[0x2A], u16::from_le_bytes([mem[0x10], mem[0x11]]), u16::from_le_bytes([mem[0x12], mem[0x13]]));
    for ch in 0..17 {
        match d.pick(ch, &c, &mem) {
            Some(r) => {
                let t = if ch < 6 { &p1 } else { &p2 };
                println!("{ch:2}: cond {:3} phrase {:3} act {:2} voice {:03X}: {}", r.condition, r.phrase, r.action(),
                    r.phrase - d.first_phrase(ch) + 1, dune_formats::text::to_unicode(t.get(r.phrase as usize - 1)));
            }
            None => println!("{ch:2}: (nothing)"),
        }
    }
}

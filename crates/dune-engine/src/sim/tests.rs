//! Tests of the simulation against the original code.
//!
//! The expected values were produced by running the original routines of DNCDPRG.EXE under
//! an x86 emulator (unicorn harness of docs/re/troops.md §5 / war.md §9:
//! scratchpad/troops/temu.py, scratchpad/war/t1.py .. t4.py, scratchpad/simtest/slots.py).

use super::*;

fn blank() -> (Vec<u8>, Vec<u8>) {
    (vec![0u8; 0x10000], vec![0u8; 0xC5FC])
}

/// The game-start state (EXE data segment, TABLAT, MAP, sub_10169), or None without data.
fn start_state() -> Option<(Vec<u8>, Vec<u8>)> {
    let dat = std::env::var("DUNE_DAT").ok()?;
    let assets = crate::assets::Assets::open(std::path::Path::new(&dat), 1).ok()?;
    let exe = assets.exe.as_ref()?;
    let mut mem = exe[crate::assets::EXE_DS..].to_vec();
    mem.resize(0x10000, 0);
    let mut map = assets.dat.read("MAP.HSQ").ok()?;
    map.resize(dune_formats::save::MAP_SIZE, 0);
    let map2 = assets.dat.read("MAP2.HSQ").ok()?;
    planet::init_tables(&mut mem, &assets.dat.read("TABLAT.BIN").ok()?);
    planet::init_locations(&mut mem, &mut map, &map2, true);
    let mut l = 1;
    Sim::new(&mut mem, &mut map, &mut l).init_troops();
    Some((mem, map))
}

fn troop(i: u8) -> usize {
    troop_addr(i)
}
fn loc(i: usize) -> usize {
    0xE4 + 0x1C * i
}

#[test]
fn rngs() {
    let (mut m, mut map) = blank();
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    s.sw(0xD826, 0x5678);
    // p = 0x5678 * 0xCBD1 = 0x44D7BFF8 -> seed BFF9, ax = (D7 << 8) | BF
    assert_eq!(s.rnd(), 0xD7BF);
    assert_eq!(s.w(0xD826), 0xBFF9);
    s.sw(0xD824, 0x1234);
    // 0x1234 * 0xE56D = 0x10504424
    let v = s.rnd2(0xFFFF);
    assert_eq!(s.w(0xD824), 0x4425);
    assert_eq!(v, 0x5044);
    assert_eq!(s.rndr(0), 0);
}

/// War.md §4.1 examples and the t1.py validation (strength, damage, threshold).
#[test]
fn strength_damage_threshold() {
    let (mut m, mut map) = blank();
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    let t = troop(1);
    // A fresh Fremen troop (mot 28, mil 10, men 190, no equipment): 3.
    s.sb(t + 3, 2);
    s.sb(t + 0x15, 28);
    s.sb(t + 0x17, 10);
    s.sb(t + 0x1A, 190);
    assert_eq!(s.strength(t), 3);
    // A typical garrison troop (80/80/0x38/180): 158.
    s.sb(t + 3, 0x8C);
    s.sb(t + 0x15, 80);
    s.sb(t + 0x17, 80);
    s.sb(t + 0x19, 0x38);
    s.sb(t + 0x1A, 180);
    assert_eq!(s.strength(t), 158);
    // Threshold: F = H -> 128; F >= 1.97 H -> 252; H >= 1.97 F -> 4; F = 0 -> 4; H = 0 -> 252.
    assert_eq!(Sim::threshold(1000, 1000), 128);
    assert_eq!(Sim::threshold(1000, 1970), 0xFC);
    assert_eq!(Sim::threshold(1970, 1000), 4);
    assert_eq!(Sim::threshold(5, 0), 4);
    assert_eq!(Sim::threshold(0, 5), 0xFC);
    // Damage: mil 95 -> factor 65/255.
    s.sb(t + 0x17, 95);
    s.sb(t + 0x1A, 200);
    assert_eq!(s.damage(t, 200), ((200 * 65) >> 8) as u8);
    assert_eq!(s.damage(t, 0xFFFF), 200);
}

include!("vectors.rs");

#[test]
fn emulated_vectors() {
    let (mut m, mut map) = blank();
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    // t1.py: strength (sub_1342D), damage (sub_1758D), threshold (sub_133D9).
    for (k, v) in T1.iter().enumerate() {
        let t = troop(1);
        for (i, &b) in v.rec.iter().enumerate() {
            s.sb(t + i, b);
        }
        s.sb(0xFA, v.fa);
        s.sb(0x2A, v.ph);
        s.sw(0x114E, v.cur);
        assert_eq!(s.strength(t), v.strength as u16, "strength case {k}");
        assert_eq!(s.damage(t, v.dx), v.damage, "damage case {k}");
        assert_eq!(Sim::threshold(v.h, v.f), v.ratio, "threshold case {k}");
    }
    // t4.py: battle gauge (sub_160F8).
    for (k, g) in T4.iter().enumerate() {
        let p = 0x138;
        for (i, t) in g.troops.iter().enumerate() {
            let a = troop(i as u8 + 1);
            for b in 0..27 {
                s.sb(a + b, 0);
            }
            s.sb(a, i as u8 + 1);
            s.sb(a + 1, if i + 1 < g.troops.len() { i as u8 + 2 } else { 0 });
            s.sb(a + 3, t.0);
            s.sw(a + 4, p as u16);
            s.sb(a + 0x10, if t.1 { 0x80 } else { 0 });
            s.sb(a + 0x1A, t.2);
            s.sw(a + 0x0C, t.3);
            s.sw(a + 0x0E, t.4);
        }
        s.sb(p + 9, 1);
        assert_eq!(s.gauge(p), g.gauge, "gauge case {k}");
    }
    // Emperor demand (loc_120D2) and shipment (sub_12566).
    for (k, d) in DEMAND.iter().enumerate() {
        s.sb(0xC3, d.n);
        s.sb(0xBE, d.be);
        s.sw(0xD824, d.seed);
        s.sb(0xC8, 0);
        s.new_demand(0);
        assert_eq!((s.w(0xBC), s.w(0xD824)), (d.bc, d.seed_out), "demand case {k}");
    }
    for (k, d) in SHIP.iter().enumerate() {
        s.sw(0xA0, d.a0);
        s.sw(0xBC, d.bc);
        s.sb(0xBF, d.bf);
        s.sb(0xC3, d.c3);
        s.sw(0x118D, d.d118);
        s.sw(2, d.t);
        s.sw(0xD824, d.seed);
        s.sw(0x1172, 0);
        s.ship(d.amount);
        assert_eq!(
            (s.b(0xBE), s.b(0xBF), s.w(0x118D), s.b(0xCF), s.w(0xA0), s.w(0x1172)),
            (d.be, d.bf_out, d.d118_out, d.cf, d.a0.wrapping_sub(d.amount), d.amount),
            "ship case {k}"
        );
    }
    for (k, o) in OFFERS.iter().enumerate() {
        s.sw(0xBC, o.bc);
        s.sb(0xBF, 0x90);
        s.offers(o.stock);
        assert_eq!(([s.w(0xB4), s.w(0xB6), s.w(0xB8), s.w(0xBA)], s.b(0xBF)), (o.out, o.bf), "offers case {k}");
    }
}

/// Mining, prospecting and training against the original (val_mine.py / val_misc.py).
#[test]
fn emulated_jobs() {
    let (mut m, mut map) = blank();
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    let t = troop(1);
    let p = 0x250;
    for (k, c) in JOBS.iter().enumerate() {
        for a in [t, p] {
            for b in 0..0x1C {
                s.sb(a + b, 0);
            }
        }
        for (i, &b) in c.rec.iter().enumerate() {
            s.sb(t + i, b);
        }
        s.sw(t + 4, p as u16);
        for (i, &b) in c.place.iter().enumerate() {
            s.sb(p + i, b);
        }
        s.sb(p + 9, 1);
        s.sb(0xFA, c.fa);
        s.sb(0x2A, c.ph);
        s.sw(0x114E, 0);
        s.sw(0x46E1, c.r46e1);
        s.sw(0xA0, c.a0);
        s.sw(2, c.t);
        s.sb(0x46DE, 0);
        s.sb(0x0A, 0);
        s.sb(0x46EB, 0);
        s.sw(0x473C, 0);
        s.sw(0x46EF, 0);
        s.sw(0x11D3, 0);
        s.sb(0x1190, 0);
        match c.job {
            0 => s.mine(t, p),
            1 => s.prospect(t, p),
            _ => s.train(t, p),
        }
        let got_t: Vec<u8> = (0..27).map(|i| s.b(t + i)).collect();
        let got_p: Vec<u8> = (0..28).map(|i| s.b(p + i)).collect();
        assert_eq!(got_t, c.rec_out.to_vec(), "job {} case {k} troop", c.job);
        assert_eq!(got_p, c.place_out.to_vec(), "job {} case {k} place", c.job);
        assert_eq!((s.w(0xA0), s.w(0x46E1)), (c.a0_out, c.r46e1_out), "job {} case {k} stock", c.job);
    }
}

/// Charisma propagation over the 67 troops (val_char.py), from the game-start state.
#[test]
fn charisma() {
    let Some((mut m, mut map)) = start_state() else { return };
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    for i in [1u8, 2, 4, 7, 9] {
        s.rally(troop(i));
    }
    let before: Vec<u8> = (1..=67).map(|i| s.b(troop(i) + 0x15)).collect();
    s.sb(0x29, 98);
    s.charisma_up(7);
    assert_eq!(s.b(0x29), 105);
    // 98 & ~3 = 96, 105 & ~3 = 104: two buckets, +2 for active troops only.
    for i in 1..=67u8 {
        let t = troop(i);
        let exp = if s.b(t + 3) & 0xA0 == 0 { (before[i as usize - 1] + 2).min(100) } else { before[i as usize - 1] };
        assert_eq!(s.b(t + 0x15), exp, "troop {i}");
    }
    s.charisma_down(200);
    assert_eq!(s.b(0x29), 1);
}

#[test]
fn initial_troops_and_stats() {
    let Some((mut m, mut map)) = start_state() else { return };
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    // troops.md §1.3: troop 1 at Carthag-Tuek, region 2, clan 0, spotted (0x40).
    let t = troop(1);
    assert_eq!(s.w(t + 4), 0x250);
    assert_eq!(s.b(t + 0x12), 0x42);
    assert_eq!(s.b(troop(4) + 0x12), 0x84);
    assert_eq!(s.b(troop(67) + 0x12), 0x01);
    // §3.8: A4 = 21, A2 = 1, AA = 0, AC = 7321 at game start.
    s.stats();
    assert_eq!((s.w(0xA2), s.w(0xA4), s.w(0xAA), s.w(0xAC)), (1, 21, 0, 7321));
}

/// war.md §3.5: the disc spans of sub_1644E (radius 1, 5 and 12).
#[test]
fn territory_disc() {
    let spans = |r: u16| {
        let mut rows = std::collections::BTreeMap::new();
        Sim::circle(r, |bx, start, n| {
            let e: &mut (i32, i32) = rows.entry(bx).or_insert((i32::MAX, i32::MIN));
            e.0 = e.0.min(start);
            e.1 = e.1.max(start + n - 1);
        });
        rows
    };
    let r1 = spans(1);
    for dr in -2..=1 {
        assert_eq!(r1[&dr], (-2, 2), "r=1 row {dr}");
    }
    let r5 = spans(5);
    let exp5 = [(-5, 3), (-4, 5), (-3, 5), (-2, 6), (-1, 6), (0, 6), (1, 6), (2, 5), (3, 5), (4, 3)];
    for (dr, w) in exp5 {
        assert_eq!(r5[&dr], (-w, w), "r=5 row {dr}");
    }
    assert_eq!(r5.len(), exp5.len());
    let r12 = spans(12);
    assert_eq!(r12[&-12], (-5, 5));
    assert_eq!(r12[&0], (-13, 13));
    assert_eq!(r12[&11], (-5, 5));
}

fn fnv(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for &x in b {
        h ^= x as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len() / 2).map(|i| u8::from_str_radix(&s[2 * i..2 * i + 2], 16).unwrap()).collect()
}

/// Whole per-slot simulation (sub_11B23 from seg000:1B46) against the original, from the
/// game-start state with a scripted scenario (scratchpad/simtest/slots.py).
fn run_slots(which: u8, fixture: &str) {
    let Some((mut m, mut map)) = start_state() else { return };
    let mut l = 1;
    let mut s = Sim::new(&mut m, &mut map, &mut l);
    s.sw(0xD824, 0x1234);
    s.sw(0xD826, 0x5678);
    s.sw(0xD828, 0x9ABC);
    s.sw(0, 0x4321);
    s.sb(0x2A, 0x40);
    s.sb(0x0A, 0x21);
    s.sw(0x1154, 0);
    s.sw(0x114E, 0x100);
    s.sw(0x1150, 0x100);
    if which == 0 {
        for i in [1u8, 2, 3, 4, 5, 6, 13, 14] {
            s.rally(troop(i));
        }
        s.sb(troop(1) + 0x19, 0x80);
        s.sb(troop(4) + 0x19, 0xC0);
        for (i, job) in [(1u8, 0u8), (2, 4), (3, 1), (4, 0), (5, 9), (6, 8), (13, 4), (14, 0)] {
            s.set_job(troop(i), job);
        }
        s.sb(troop(6) + 0x19, 2);
        for i in [13, 14, 16, 24, 23] {
            let v = s.b(loc(i) + 0x0A) | 0x40;
            s.sb(loc(i) + 0x0A, v);
        }
        let v = s.b(loc(24) + 0x0A) | 0x20;
        s.sb(loc(24) + 0x0A, v);
        s.sb(loc(24) + 0x1B, 100);
    } else if which == 2 {
        for i in [1u8, 2, 3, 4] {
            s.rally(troop(i));
        }
        for i in [1u8, 2, 4] {
            let t = troop(i);
            s.sb(t + 0x19, 0x3C);
            s.sb(t + 0x1A, 250);
            s.sb(t + 0x17, 90);
            s.sb(t + 0x15, 100);
            s.set_job(t, 6);
            s.depart(t, loc(44));
        }
        s.sw(0x11D3, loc(14) as u16);
        s.sw(0x11D5, loc(11) as u16);
        s.sw(0x11D7, 0);
        s.set_job(troop(3), 1);
        s.sb(0x2A, 0x50);
    } else {
        for i in [1u8, 2, 8, 9] {
            s.rally(troop(i));
        }
        s.sb(troop(1) + 0x19, 0x3C);
        s.sb(troop(2) + 0x19, 0x38);
        s.sb(troop(1) + 0x1A, 250);
        s.sb(troop(2) + 0x1A, 250);
        s.sb(troop(1) + 0x17, 80);
        s.sb(troop(2) + 0x17, 70);
        s.sb(troop(1) + 0x15, 90);
        s.sb(troop(2) + 0x15, 90);
        for i in [1u8, 2] {
            s.set_job(troop(i), 6);
            s.depart(troop(i), loc(3));
        }
        s.set_job(troop(8), 5);
        s.depart(troop(8), loc(32));
        s.sb(0x2A, 0x3C);
    }
    let mut checks = fixture.lines().map(|l| {
        let mut f = l.split(' ');
        let k: usize = f.next().unwrap().parse().unwrap();
        (k, hex(f.next().unwrap()), hex(f.next().unwrap()), u64::from_str_radix(f.next().unwrap(), 16).unwrap())
    });
    let mut next = checks.next();
    for k in 1..=160 {
        let r = s.rnd();
        s.sw(0, r);
        let t = s.clock().wrapping_add(1);
        s.sw(2, t);
        s.sb(0x46DD, 0);
        s.slot();
        if let Some((ck, ds, seeds, mh)) = &next {
            if *ck == k {
                let diffs: Vec<String> = ds.iter().enumerate().filter(|(a, &b)| s.b(*a) != b).map(|(a, &b)| format!("{a:04X}:{:02X}!={b:02X}", s.b(a))).take(40).collect();
                assert!(diffs.is_empty(), "scenario {which}, slot {k}: {}", diffs.join(" "));
                for (i, &b) in seeds.iter().enumerate() {
                    assert_eq!(s.b(0xD824 + i), b, "scenario {which}, slot {k}: seed byte {i}");
                }
                assert_eq!(fnv(&s.map[..MAP_CELLS]), *mh, "scenario {which}, slot {k}: map");
                next = checks.next();
            }
        }
    }
}

#[test]
fn slots_economy() {
    run_slots(0, include_str!("slots0.txt"));
}

#[test]
fn slots_war() {
    run_slots(1, include_str!("slots1.txt"));
}

#[test]
fn slots_victory() {
    run_slots(2, include_str!("slots2.txt"));
}

/// MASSIVE ATTACK (seg000:7317 up to the flash loop) against the original
/// (scratchpad/simtest/massive.py): Harkonnen strikes, and Fremen strikes up to a liberation.
#[test]
fn massive_attack() {
    let Some((mut m, mut map)) = start_state() else { return };
    let mut l = 1;
    for line in include_str!("massive.txt").lines() {
        let mut f = line.split(' ');
        let k = f.next().unwrap();
        let (pre, post) = (hex(f.next().unwrap()), hex(f.next().unwrap()));
        let mut s = Sim::new(&mut m, &mut map, &mut l);
        for (a, &b) in pre[..0x1261].iter().enumerate() {
            s.sb(a, b);
        }
        for (i, &b) in pre[0x1261..].iter().enumerate() {
            s.sb(0xD824 + i, b);
        }
        s.massive_attack();
        let diffs: Vec<String> = post[..0x1261].iter().enumerate().filter(|(a, &b)| s.b(*a) != b).map(|(a, &b)| format!("{a:04X}:{:02X}!={b:02X}", s.b(a))).take(40).collect();
        assert!(diffs.is_empty(), "case {k}: {}", diffs.join(" "));
        for (i, &b) in post[0x1261..].iter().enumerate() {
            assert_eq!(s.b(0xD824 + i), b, "case {k}: seed byte {i}");
        }
    }
}

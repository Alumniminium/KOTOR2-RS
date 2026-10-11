//! Print key fields of a UTC. Usage: utcinfo RESREF [MODULE]
use k2_data::{Creature, Tables};
use k2_res::{Install, Resources};

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into())).unwrap();
    let mut res = Resources::open(install).unwrap();
    if let Some(m) = a.get(2) {
        res.load_module(m).unwrap();
    }
    let c = Creature::load(&res, &a[1]).unwrap();
    println!("tag {:?} conversation {:?} hologram {} appearance {}", c.tag, c.conversation, c.hologram, c.appearance);
    if let Ok(m) = c.models(&res, &Tables::new()) {
        println!("body {} head {:?} tex {:?}", m.body, m.head, m.body_texture);
    }
}

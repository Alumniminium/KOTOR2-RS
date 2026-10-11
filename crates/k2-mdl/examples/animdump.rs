//! Dump a model's own animations: per node, controller types and first/last
//! key values. Usage: animdump MODEL.mdl MODEL.mdx [ANIM]
use k2_mdl::{MdlAnimNode, read_mdl_from_bytes};

fn walk(n: &MdlAnimNode, depth: usize) {
    for c in &n.controllers {
        let (f, l) = (c.keys.first(), c.keys.last());
        println!(
            "{:indent$}{} #{} {:?} keys={} first={:?} last={:?}",
            "",
            n.name,
            n.node_number,
            c.controller_type,
            c.keys.len(),
            f.map(|k| (k.time, k.values.clone())),
            l.map(|k| (k.time, k.values.clone())),
            indent = depth * 2
        );
    }
    for ch in &n.children {
        walk(ch, depth + 1);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mdl = std::fs::read(&a[1]).unwrap();
    let mdx = std::fs::read(&a[2]).ok();
    let m = read_mdl_from_bytes(&mdl, mdx.as_deref()).unwrap();
    for anim in &m.animations {
        if a.get(3).is_some_and(|w| !anim.name.eq_ignore_ascii_case(w)) {
            println!("anim {} len {} root {}", anim.name, anim.length, anim.anim_root);
            continue;
        }
        println!("anim {} len {} root {} events {:?}", anim.name, anim.length, anim.anim_root, anim.events.iter().map(|e| (e.time, &e.name)).collect::<Vec<_>>());
        walk(&anim.root_node, 1);
    }
}

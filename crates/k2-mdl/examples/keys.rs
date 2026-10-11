//! Print every key of a node's controllers in one animation:
//! keys MDL MDX ANIM NODE
use k2_mdl::{MdlAnimNode, read_mdl_from_bytes};

fn find<'a>(n: &'a MdlAnimNode, name: &str) -> Option<&'a MdlAnimNode> {
    if n.name.eq_ignore_ascii_case(name) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find(c, name))
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mdl = std::fs::read(&a[1]).unwrap();
    let mdx = std::fs::read(&a[2]).ok();
    let m = read_mdl_from_bytes(&mdl, mdx.as_deref()).unwrap();
    let anim = m.animations.iter().find(|x| x.name.eq_ignore_ascii_case(&a[3])).unwrap();
    let n = find(&anim.root_node, &a[4]).unwrap();
    for c in &n.controllers {
        println!("{:?} raw_cols {:?}", c.controller_type, c.keys.first().map(|k| k.values.len()));
        for k in &c.keys {
            println!("  {:8.3} {:?}", k.time, k.values.iter().map(|v| format!("{v:.3}")).collect::<Vec<_>>());
        }
    }
}

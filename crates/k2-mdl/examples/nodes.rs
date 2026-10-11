//! Print a model's node tree: name, type, position, rotation (w x y z).
use k2_mdl::{MdlNode, read_mdl_from_bytes};

fn walk(n: &MdlNode, d: usize) {
    let kind = format!("{:?}", n.node_data).chars().take_while(|c| c.is_alphanumeric()).collect::<String>();
    let extra = n.node_data.mesh().map_or(String::new(), |m| format!(" tex {:?}/{:?} bsphere {:.1} bbox {:?}..{:?} render {}", m.texture_0, m.texture_1, m.bsphere_radius, m.bounding_min, m.bounding_max, m.render));
    println!("{:indent$}{} [{}] pos {:?} rot {:?}{}", "", n.name, kind, n.position, n.rotation, extra, indent = d * 2);
    for c in &n.children {
        walk(c, d + 1);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mdl = std::fs::read(&a[1]).unwrap();
    let mdx = std::fs::read(&a[2]).ok();
    let m = read_mdl_from_bytes(&mdl, mdx.as_deref()).unwrap();
    println!("supermodel {:?} anims {}", m.supermodel_name, m.animations.len());
    walk(&m.root_node, 0);
}

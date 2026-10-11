//! List mesh nodes with render and K2 hologram flags. Usage: meshflags MODEL.mdl MODEL.mdx
use k2_mdl::{MdlNode, MdlNodeData, read_mdl_from_bytes};

fn walk(n: &MdlNode) {
    let mesh = match &n.node_data {
        MdlNodeData::Mesh(m) => Some(m),
        MdlNodeData::Skin(s) => Some(&s.mesh),
        MdlNodeData::Dangly(d) => Some(&d.mesh),
        MdlNodeData::AnimMesh(a) => Some(&a.mesh),
        _ => None,
    };
    if let Some(m) = mesh {
        println!("{:24} render={} faces={} tex={:?} uv={}/{},{}/{},{} tsl={:?}", n.name, m.render, m.faces.len(), m.texture_0, m.animate_uv, m.uv_direction_x, m.uv_direction_y, m.uv_jitter, m.uv_jitter_speed, m.tsl);
    }
    for c in &n.children {
        walk(c);
    }
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let mdl = std::fs::read(&a[1]).unwrap();
    let mdx = std::fs::read(&a[2]).ok();
    let m = read_mdl_from_bytes(&mdl, mdx.as_deref()).unwrap();
    walk(&m.root_node);
}

//! Checks, for every room in every module, that WOK vertices equal the room
//! MDL's AABB-node vertices transformed by LYT position + node position.
use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

fn find(n: &k2_mdl::MdlNode, acc: [f32; 3]) -> Option<(&k2_mdl::MdlNode, [f32; 3])> {
    let p = [acc[0] + n.position[0], acc[1] + n.position[1], acc[2] + n.position[2]];
    if let k2_mdl::MdlNodeData::Aabb(_) = &n.node_data { return Some((n, p)); }
    n.children.iter().find_map(|c| find(c, p))
}

fn main() {
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or("..".into())).unwrap();
    let mods = install.module_names();
    let mut res = Resources::open(install).unwrap();
    let (mut ok, mut bad, mut rot, mut nowok) = (0, 0, 0, 0);
    for module in mods {
        res.load_module(&module).unwrap();
        let Some(lyt) = res.get(&ResKey::new(&module, ResourceType::Lyt)).unwrap() else { continue };
        let lyt = rakata_formats::read_lyt_from_bytes(&lyt).unwrap();
        for room in &lyt.rooms {
            let Some(mdl) = res.get(&ResKey::new(&room.model, ResourceType::Mdl)).unwrap() else { continue };
            let mdx = res.get(&ResKey::new(&room.model, ResourceType::Mdx)).unwrap();
            let m = k2_mdl::read_mdl_from_bytes(&mdl, mdx.as_deref()).unwrap();
            let Some(wok) = res.get(&ResKey::new(&room.model, ResourceType::Wok)).unwrap() else { nowok += 1; continue };
            let w = rakata_formats::read_bwm_from_bytes(&wok).unwrap();
            let Some((node, np)) = find(&m.root_node, [0.0; 3]) else { continue };
            if node.rotation != [1.0, 0.0, 0.0, 0.0] { rot += 1; }
            let k2_mdl::MdlNodeData::Aabb(a) = &node.node_data else { unreachable!() };
            if a.mesh.positions.is_empty() || w.vertices.is_empty() { continue; }
            let matched = a.mesh.positions.iter().filter(|v| {
                let t = [v[0] + np[0] + room.position.x, v[1] + np[1] + room.position.y, v[2] + np[2] + room.position.z];
                w.vertices.iter().any(|x| (x.x - t[0]).abs() < 0.01 && (x.y - t[1]).abs() < 0.01 && (x.z - t[2]).abs() < 0.01)
            }).count();
            let all = matched == a.mesh.positions.len();
            let hdr = (w.position.x - np[0]).abs() < 0.01 && (w.position.y - np[1]).abs() < 0.01 && (w.position.z - np[2]).abs() < 0.01;
            if all { ok += 1 } else { bad += 1; println!("mismatch {module} {} hdr_eq_nodepos={hdr} matched {matched}/{} wokverts {}", room.model, a.mesh.positions.len(), w.vertices.len()) }
        }
    }
    println!("world-space match {ok}, mismatch {bad}, rotated aabb nodes {rot}, rooms without wok {nowok}");
}

//! Compares a room's AABB-node mesh (in the MDL) with its WOK vertices.
use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

fn find(n: &k2_mdl::MdlNode) -> Option<&k2_mdl::MdlNode> {
    if let k2_mdl::MdlNodeData::Aabb(_) = &n.node_data { return Some(n); }
    n.children.iter().find_map(find)
}

fn main() {
    let module = std::env::args().nth(1).unwrap_or_else(|| "101per".into());
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or("..".into())).unwrap();
    let mut res = Resources::open(install).unwrap();
    res.load_module(&module).unwrap();
    let lyt = rakata_formats::read_lyt_from_bytes(&res.get(&ResKey::new(&module, ResourceType::Lyt)).unwrap().unwrap()).unwrap();
    for room in lyt.rooms.iter().take(6) {
        let mdl = res.get(&ResKey::new(&room.model, ResourceType::Mdl)).unwrap().unwrap();
        let mdx = res.get(&ResKey::new(&room.model, ResourceType::Mdx)).unwrap();
        let m = k2_mdl::read_mdl_from_bytes(&mdl, mdx.as_deref()).unwrap();
        let Some(wok) = res.get(&ResKey::new(&room.model, ResourceType::Wok)).unwrap() else { continue };
        let w = rakata_formats::read_bwm_from_bytes(&wok).unwrap();
        let Some(node) = find(&m.root_node) else { println!("{} no aabb", room.model); continue };
        let k2_mdl::MdlNodeData::Aabb(a) = &node.node_data else { unreachable!() };
        let mv = &a.mesh.positions;
        println!("{:<10} lyt ({:.2},{:.2},{:.2}) node pos {:?} rot {:?} mdl verts {} wok verts {} faces mdl {} wok {}", room.model, room.position.x, room.position.y, room.position.z,
            node.position, node.rotation, mv.len(), w.vertices.len(), a.mesh.faces.len(), w.faces.len());
        for i in 0..mv.len().min(w.vertices.len()).min(3) {
            let wv = w.vertices[i];
            println!("   v{i} mdl {:?} wok ({:.3},{:.3},{:.3}) wok-mdl ({:.3},{:.3},{:.3})", mv[i], wv.x, wv.y, wv.z, wv.x - mv[i][0], wv.y - mv[i][1], wv.z - mv[i][2]);
        }
    }
}

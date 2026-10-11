//! Prints LYT room positions next to WOK bounds and header fields for a module.
use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

fn main() {
    let module = std::env::args().nth(1).unwrap_or_else(|| "101per".into());
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or("..".into())).unwrap();
    let mut res = Resources::open(install).unwrap();
    res.load_module(&module).unwrap();
    let lyt = res.get(&ResKey::new(&module, ResourceType::Lyt)).unwrap().unwrap();
    let lyt = rakata_formats::read_lyt_from_bytes(&lyt).unwrap();
    for room in &lyt.rooms {
        let Some(b) = res.get(&ResKey::new(&room.model, ResourceType::Wok)).unwrap() else {
            println!("{:<12} pos ({:8.2},{:8.2},{:8.2}) NO WOK", room.model, room.position.x, room.position.y, room.position.z);
            continue;
        };
        let w = rakata_formats::read_bwm_from_bytes(&b).unwrap();
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for v in &w.vertices {
            for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() {
                mn[i] = mn[i].min(c);
                mx[i] = mx[i].max(c);
            }
        }
        println!(
            "{:<12} pos ({:8.2},{:8.2},{:8.2}) wok min ({:8.2},{:8.2},{:8.2}) max ({:8.2},{:8.2},{:8.2}) type {} hdrpos ({:.2},{:.2},{:.2}) faces {} aabb {} adj {} edges {} perim {}",
            room.model, room.position.x, room.position.y, room.position.z,
            mn[0], mn[1], mn[2], mx[0], mx[1], mx[2],
            w.walkmesh_type.raw(), w.position.x, w.position.y, w.position.z,
            w.faces.len(), w.aabb_nodes.len(), w.adjacencies.len(), w.edges.len(), w.perimeters.len()
        );
    }
    let ifo = res.get(&ResKey::new("module", ResourceType::Ifo)).unwrap().unwrap();
    let ifo = rakata_formats::read_gff_from_bytes(&ifo).unwrap();
    for f in ["Mod_Entry_X", "Mod_Entry_Y", "Mod_Entry_Z", "Mod_Entry_Dir_X", "Mod_Entry_Dir_Y"] {
        println!("{f} = {:?}", ifo.root.field(f));
    }
    let sm = res.get(&ResKey::new("surfacemat", ResourceType::TwoDa)).unwrap().unwrap();
    let sm = rakata_formats::read_twoda_from_bytes(&sm).unwrap();
    println!("surfacemat headers {:?}", sm.headers);
    for i in 0..sm.row_count() {
        println!("  {i:2} {:<14} walk={:?} lineofsight={:?} grass={:?}", sm.cell(i, "label").unwrap_or("-"), sm.cell(i, "walk"), sm.cell(i, "lineofsight"), sm.cell(i, "grass"));
    }
}

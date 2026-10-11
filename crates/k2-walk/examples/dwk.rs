//! Dumps door/placeable walkmesh headers and bounds.
use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

fn main() {
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or("..".into())).unwrap();
    let res = Resources::open(install).unwrap();
    let names: Vec<String> = std::env::args().skip(1).collect();
    for n in names {
        let (stem, ext) = n.rsplit_once('.').unwrap();
        let ty = ResourceType::from_extension(ext);
        let Some(b) = res.get(&ResKey::new(stem, ty)).unwrap() else { println!("{n}: missing"); continue };
        let w = rakata_formats::read_bwm_from_bytes(&b).unwrap();
        let mut mn = [f32::MAX; 3];
        let mut mx = [f32::MIN; 3];
        for v in &w.vertices { for (i, c) in [v.x, v.y, v.z].into_iter().enumerate() { mn[i] = mn[i].min(c); mx[i] = mx[i].max(c); } }
        let mats: std::collections::BTreeMap<u32, usize> = w.faces.iter().fold(Default::default(), |mut m, f| { *m.entry(f.material_id).or_default() += 1; m });
        println!("{n}: type {} faces {} aabb {} min ({:.2},{:.2},{:.2}) max ({:.2},{:.2},{:.2}) pos ({:.2},{:.2},{:.2}) rh1 ({:.2},{:.2},{:.2}) rh2 ({:.2},{:.2},{:.2}) ah1 ({:.2},{:.2},{:.2}) ah2 ({:.2},{:.2},{:.2}) mats {:?}",
            w.walkmesh_type.raw(), w.faces.len(), w.aabb_nodes.len(), mn[0], mn[1], mn[2], mx[0], mx[1], mx[2], w.position.x, w.position.y, w.position.z,
            w.relative_hook1.x, w.relative_hook1.y, w.relative_hook1.z, w.relative_hook2.x, w.relative_hook2.y, w.relative_hook2.z,
            w.absolute_hook1.x, w.absolute_hook1.y, w.absolute_hook1.z, w.absolute_hook2.x, w.absolute_hook2.y, w.absolute_hook2.z, mats);
    }
}

//! For every door model with DWKs, reports open states whose blocking faces
//! still intrude on the doorway (|x| < radius, |y| < 1 in door-local space).
use std::collections::BTreeSet;

use k2_res::{Install, ResKey, Resources};
use k2_walk::Materials;
use rakata_core::ResourceType;

fn main() {
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or("..".into())).unwrap();
    let res = Resources::open(install).unwrap();
    let mats = Materials::from_2da_bytes(&res.get(&ResKey::new("surfacemat", ResourceType::TwoDa)).unwrap().unwrap()).unwrap();
    let radius: f32 = std::env::var("R").ok().and_then(|s| s.parse().ok()).unwrap_or(0.3);
    let mut models = BTreeSet::new();
    for k in res.chitin_keys().chain(res.override_keys()) {
        if k.extension() == "dwk" {
            let n = k.name.clone();
            if let Some(stem) = n.strip_suffix(['0', '1', '2']) {
                models.insert(stem.to_owned());
            }
        }
    }
    println!("{} door models", models.len());
    for m in models {
        let mut line = String::new();
        for i in 0..3 {
            let Some(b) = res.get(&ResKey::new(&format!("{m}{i}"), ResourceType::Dwk)).unwrap() else { line += &format!(" s{i}=none"); continue };
            let w = rakata_formats::read_bwm_from_bytes(&b).unwrap();
            let p = w.position;
            let mut hit = 0;
            for f in &w.faces {
                if mats.walkable(f.material_id) {
                    continue;
                }
                let v: Vec<(f32, f32)> = f.vertex_indices.iter().map(|&k| { let v = w.vertices[k as usize]; (v.x + p.x, v.y + p.y) }).collect();
                let (x0, x1) = (v.iter().map(|a| a.0).fold(f32::MAX, f32::min), v.iter().map(|a| a.0).fold(f32::MIN, f32::max));
                let (y0, y1) = (v.iter().map(|a| a.1).fold(f32::MAX, f32::min), v.iter().map(|a| a.1).fold(f32::MIN, f32::max));
                if x1 > -radius && x0 < radius && y1 > -1.0 && y0 < 1.0 {
                    hit += 1;
                }
            }
            line += &format!(" s{i}={}/{}", hit, w.faces.len());
        }
        println!("{m}:{line}");
    }
}

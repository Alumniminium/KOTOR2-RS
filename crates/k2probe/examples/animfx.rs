//! Survey of model effects the renderer must animate by itself: clips with
//! alpha (132) or self-illumination (100) tracks, meshes with animate_uv,
//! emitter nodes, and TXI procedural textures on mesh textures.
//! Usage: K2_INSTALL=DIR animfx [MODEL_PREFIX]
use std::collections::{BTreeMap, BTreeSet};

use k2_mdl::{MdlAnimNode, MdlControllerType, MdlNode, MdlNodeData, read_mdl_from_bytes};
use k2_res::{Install, ResKey, Resources};
use rakata_core::ResourceType;

fn anim_has(n: &MdlAnimNode, ty: MdlControllerType, out: &mut BTreeSet<String>) {
    if n.controllers.iter().any(|c| c.controller_type == ty && c.keys.len() > 1) {
        out.insert(n.name.clone());
    }
    for c in &n.children {
        anim_has(c, ty, out);
    }
}

fn walk(n: &MdlNode, tex: &mut BTreeSet<String>, uv: &mut usize, emit: &mut usize) {
    let mesh = match &n.node_data {
        MdlNodeData::Mesh(m) => Some(m),
        MdlNodeData::Skin(s) => Some(&s.mesh),
        MdlNodeData::Dangly(d) => Some(&d.mesh),
        MdlNodeData::AnimMesh(a) => Some(&a.mesh),
        MdlNodeData::Emitter(_) => {
            *emit += 1;
            None
        }
        _ => None,
    };
    if let Some(m) = mesh {
        if m.animate_uv != 0 {
            *uv += 1;
        }
        if m.render && !m.texture_0.trim().is_empty() {
            tex.insert(m.texture_0.trim().to_ascii_lowercase());
        }
    }
    for c in &n.children {
        walk(c, tex, uv, emit);
    }
}

fn main() {
    let prefix = std::env::args().nth(1).unwrap_or_default().to_ascii_lowercase();
    let install = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "..".into())).unwrap();
    let res = Resources::open(install).unwrap();
    let mut keys: Vec<ResKey> = res.chitin_keys().cloned().collect();
    keys.sort();
    let mut procs: BTreeMap<String, String> = BTreeMap::new();
    for k in keys.iter().filter(|k| k.extension() == "mdl" && k.to_string().to_ascii_lowercase().starts_with(&prefix)) {
        let name = k.to_string();
        let stem = name.trim_end_matches(".mdl");
        let Some(mdl) = res.get(k).ok().flatten() else { continue };
        let mdx = res.get(&ResKey::new(stem, ResourceType::Mdx)).ok().flatten();
        let Ok(m) = read_mdl_from_bytes(&mdl, mdx.as_deref()) else { continue };
        let (mut tex, mut uv, mut emit) = (BTreeSet::new(), 0, 0);
        walk(&m.root_node, &mut tex, &mut uv, &mut emit);
        let (mut alpha, mut illum) = (BTreeSet::new(), BTreeSet::new());
        let mut clips = BTreeSet::new();
        for a in &m.animations {
            let (mut al, mut il) = (BTreeSet::new(), BTreeSet::new());
            anim_has(&a.root_node, MdlControllerType::ALPHA, &mut al);
            anim_has(&a.root_node, MdlControllerType::SELFILLUMCOLOR, &mut il);
            if !al.is_empty() || !il.is_empty() {
                clips.insert(a.name.clone());
            }
            alpha.extend(al);
            illum.extend(il);
        }
        let mut ptex = Vec::new();
        for t in &tex {
            let txi = res
                .get(&ResKey::new(t, ResourceType::Txi))
                .ok()
                .flatten()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .or_else(|| {
                    res.get(&ResKey::new(t, ResourceType::Tpc)).ok().flatten().map(|b| {
                        let s = String::from_utf8_lossy(&b[128.min(b.len())..]).into_owned();
                        s.find("proceduretype").map(|i| s[i..].to_owned()).unwrap_or_default()
                    })
                })
                .unwrap_or_default()
                .to_ascii_lowercase();
            if let Some(l) = txi.lines().find(|l| l.trim().starts_with("proceduretype")) {
                let kind = l.split_whitespace().nth(1).unwrap_or("?").to_owned();
                procs.insert(t.clone(), kind.clone());
                ptex.push(format!("{t}:{kind}"));
            }
        }
        if !alpha.is_empty() || !illum.is_empty() || uv > 0 || emit > 0 || !ptex.is_empty() {
            println!(
                "{stem}: alpha_nodes={} illum_nodes={} clips={:?} animate_uv={uv} emitters={emit} proc={:?}",
                alpha.len(),
                illum.len(),
                clips,
                ptex
            );
        }
    }
    let kinds: BTreeSet<&String> = procs.values().collect();
    eprintln!("procedural textures on models: {} kinds {:?}", procs.len(), kinds);
}

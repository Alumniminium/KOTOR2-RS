//! Useable placeables must be findable on screen. Regression for the 101PER
//! Security Desk (tag SecTer): its model `plc_desk1` keys a `cutscenedummy`
//! node at the origin in `default`, which the presentation layer used to
//! read as an area-space stunt pose, collapsing the desk (and its pick
//! sphere) to the area origin: invisible and not interactable.
//!
//! Skips when no install is found. The survey prints every useable
//! placeable without visible geometry; run with `-- --nocapture`.

use std::collections::BTreeMap;

use k2_mdl::{MdlAnimNode, MdlControllerType, MdlNode, MdlNodeData, read_mdl_from_bytes};
use k2_res::{Install, ResKey, Resources};
use k2_world::{ObjectKind, World};
use rakata_core::ResourceType;

fn install() -> Option<Install> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    Install::discover(dir).ok()
}

/// Drawable mesh nodes of a model (render flag set, faces present, not the
/// door-transition marker `trans`).
fn drawable(n: &MdlNode) -> usize {
    let mesh = match &n.node_data {
        MdlNodeData::Mesh(m) => Some(m),
        MdlNodeData::Skin(s) => Some(&s.mesh),
        MdlNodeData::Dangly(d) => Some(&d.mesh),
        MdlNodeData::AnimMesh(a) => Some(&a.mesh),
        _ => None,
    };
    let own = mesh.is_some_and(|m| m.render && !m.faces.is_empty() && !n.name.eq_ignore_ascii_case("trans")) as usize;
    own + n.children.iter().map(drawable).sum::<usize>()
}

/// Largest distance from the origin of any `cutscenedummy` position key.
fn dummy_offset(n: &MdlAnimNode) -> Option<f32> {
    let mut best = None;
    if n.name.eq_ignore_ascii_case("cutscenedummy") {
        for c in n.controllers.iter().filter(|c| c.controller_type == MdlControllerType::POSITION) {
            for k in c.keys.iter().filter(|k| k.values.len() >= 3) {
                let d = (k.values[0].powi(2) + k.values[1].powi(2) + k.values[2].powi(2)).sqrt();
                best = Some(best.map_or(d, |b: f32| b.max(d)));
            }
        }
    }
    n.children.iter().filter_map(dummy_offset).chain(best).reduce(f32::max)
}

#[test]
fn security_desk_is_a_normal_placed_placeable() {
    let Some(install) = install() else { return };
    let mut res = Resources::open(install.clone()).unwrap();
    let get = |res: &Resources, name: &str, ext: &str| res.get(&ResKey::new(name, ResourceType::from_extension(ext))).unwrap().unwrap();

    // The desk model has a cutscenedummy at the origin: identity, not a pose.
    let desk = read_mdl_from_bytes(&get(&res, "plc_desk1", "mdl"), Some(&get(&res, "plc_desk1", "mdx"))).unwrap();
    let default = desk.animations.iter().find(|a| a.name.eq_ignore_ascii_case("default")).expect("plc_desk1 default clip");
    assert_eq!(dummy_offset(&default.root_node), Some(0.0), "plc_desk1 default keys cutscenedummy at the origin");
    assert!(drawable(&desk.root_node) > 0, "plc_desk1 has drawable meshes");

    // The Kreia stunt really is an area-space pose.
    res.load_module("101per").unwrap();
    let stunt = read_mdl_from_bytes(&get(&res, "p_kreiastunt", "mdl"), Some(&get(&res, "p_kreiastunt", "mdx"))).unwrap();
    let cut = stunt.animations.iter().find(|a| a.name.eq_ignore_ascii_case("cut001l")).unwrap();
    assert!(dummy_offset(&cut.root_node).unwrap() > 1.0);
}

#[test]
fn security_desk_used_runs_its_conversation_and_scripts() {
    let Some(install) = install() else { return };
    let mut w = World::new(install).unwrap();
    w.enter_module("101per", None).unwrap();
    let desk = w.by_tag("SecTer", 0).expect("Security Desk in 101per");
    let o = w.get(desk).unwrap();
    assert!(o.placeable().unwrap().useable);
    assert_eq!(o.placeable().unwrap().model.as_deref(), Some("plc_desk1"));
    let (pos, wp) = (o.position, w.by_tag("WP_SECTER", 0).map(|i| w.get(i).unwrap().position).expect("WP_SECTER"));
    assert!(pos.truncate().distance(wp.truncate()) < 2.0, "{pos} vs {wp}");

    let pc = w.party_leader();
    w.set_player_position(wp, 0.0);
    w.use_object(pc, desk);
    let v = w.conversation_view().expect("using the desk starts a conversation");
    assert_eq!(v.dialog, "secter");
    assert_eq!(v.owner, desk);
    assert!(v.text.contains("SECURITY"), "{}", v.text);

    // "Return to main console functions" runs a_secterjmp, which jumps the
    // PC back to WP_SECTER: stand 3 m away first and watch it come back.
    let away = wp + glam::Vec3::new(3.0, 0.0, 0.0);
    let away = w.walk.ground(away.x, away.y, away.z + 1.0).map_or(wp, |h| glam::Vec3::new(away.x, away.y, h.z));
    let pick = |w: &mut World, text: &str| {
        let v = w.conversation_view().expect("conversation running");
        let i = v.replies.iter().position(|r| r.text.contains(text)).unwrap_or_else(|| panic!("no reply {text:?} in {v:?}"));
        w.choose_reply(i);
    };
    pick(&mut w, "security records");
    w.set_player_position(away, 0.0);
    pick(&mut w, "Return to main console");
    for _ in 0..30 {
        w.tick(1.0 / 30.0);
    }
    assert!(w.vm_errors.is_empty(), "{:?}", w.vm_errors);
    let now = w.get(pc).unwrap().position;
    assert!(now.truncate().distance(wp.truncate()) < 0.2, "a_secterjmp did not run: PC at {now}, WP_SECTER {wp}");
}

/// Every useable placeable in every module: model resolves and has drawable
/// meshes. Listed, not asserted: plc_invis and friends are invisible by
/// design (the console is baked into the room), but nothing else should be.
#[test]
fn survey_useable_placeables_have_geometry() {
    let Some(install) = install() else { return };
    let mut w = World::new(install.clone()).unwrap();
    let mut res = Resources::open(install.clone()).unwrap();
    // model -> (drawable meshes, cutscenedummy offset, uses as "module:tag useable")
    let mut models: BTreeMap<String, (usize, Option<f32>, Vec<String>)> = BTreeMap::new();
    let mut missing: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut placed: BTreeMap<String, usize> = BTreeMap::new();
    for m in install.module_names() {
        if w.enter_module(&m, None).is_err() {
            continue;
        }
        res.load_module(&m).unwrap();
        let objs: Vec<_> = w.iter().filter(|o| o.kind == ObjectKind::Placeable).map(|o| (o.tag.clone(), o.placeable().cloned().unwrap())).collect();
        for (tag, p) in objs {
            let Some(model) = p.model.clone().map(|s| s.to_ascii_lowercase()) else {
                if p.useable {
                    missing.entry("<none>".into()).or_default().push(format!("{m}:{tag}"));
                }
                continue;
            };
            *placed.entry(model.clone()).or_default() += 1;
            let e = models.entry(model.clone()).or_insert_with(|| {
                let key = |ext| ResKey::new(&model, ResourceType::from_extension(ext));
                match res.get(&key("mdl")).ok().flatten().and_then(|b| read_mdl_from_bytes(&b, res.get(&key("mdx")).ok().flatten().as_deref()).ok()) {
                    Some(mdl) => {
                        let off = mdl.animations.iter().find(|a| a.name.eq_ignore_ascii_case("default")).and_then(|a| dummy_offset(&a.root_node));
                        (drawable(&mdl.root_node), off, Vec::new())
                    }
                    None => (usize::MAX, None, Vec::new()),
                }
            });
            if p.useable {
                e.2.push(format!("{m}:{tag}"));
            }
        }
    }
    let mut collapsed = 0;
    for (model, (meshes, off, uses)) in &models {
        if *meshes == usize::MAX && !uses.is_empty() {
            missing.entry(model.clone()).or_default().extend(uses.iter().cloned());
        } else if *meshes == 0 && !uses.is_empty() {
            eprintln!("useable, no drawable mesh: {model} x{} e.g. {}", uses.len(), uses.iter().take(3).cloned().collect::<Vec<_>>().join(" "));
        }
        if let Some(d) = off
            && *d <= 0.01
        {
            collapsed += 1;
            eprintln!("identity cutscenedummy in default: {model} (placed {}, useable {})", placed[model], uses.len());
        }
    }
    for (model, uses) in &missing {
        eprintln!("useable, model not loadable: {model} {}", uses.iter().take(6).cloned().collect::<Vec<_>>().join(" "));
    }
    eprintln!("{} placeable models, {collapsed} with an identity cutscenedummy in default (were collapsed to the origin)", models.len());
    assert!(missing.is_empty(), "useable placeables whose model does not load: {missing:?}");
}

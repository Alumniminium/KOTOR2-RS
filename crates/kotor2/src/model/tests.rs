//! Model tests: mesh and skin helpers, TPC mips, fades, the lightsaber blade and the Mandalore casters.

use std::io::Write;
use super::*;

#[test]
fn lightsaber_blade_is_two_strips_of_the_saber_vertices() {
    let Some(model) = install_model("w_lghtsbr_001") else { return };
    fn find(n: &MdlNode) -> Option<&MdlSaber> {
        match &n.node_data {
            MdlNodeData::Saber(s) => Some(s),
            _ => n.children.iter().find_map(find),
        }
    }
    let saber = find(&model.root_node).expect("a saber node");
    let blade = saber_blade(saber);
    assert_eq!(blade.positions.len(), 176);
    assert_eq!(blade.uv1.len(), blade.positions.len(), "the blade has a UV per vertex");
    assert_eq!(blade.normals.len(), blade.positions.len(), "and a normal per vertex");
    assert!(blade.render, "the blade is a drawn mesh");
    let sub = |p: [f32; 3], q: [f32; 3]| [p[0] - q[0], p[1] - q[1], p[2] - q[2]];
    let mut used = std::collections::BTreeSet::new();
    for f in &blade.faces {
        let [a, b, c] = f.vertex_indices.map(|i| blade.positions[usize::from(i)]);
        let (u, v) = (sub(b, a), sub(c, a));
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let len = n.iter().map(|x| x * x).sum::<f32>().sqrt();
        assert!(len > 2e-5, "face {:?} has no area", f.vertex_indices);
        let normal = blade.normals[usize::from(f.vertex_indices[0])];
        assert!(n[0] * normal[0] + n[1] * normal[1] + n[2] * normal[2] > 0.0, "face {:?} faces away from its normal", f.vertex_indices);
        used.extend(f.vertex_indices);
    }
    // The strips are pieces 0-7 and 88-95. Every other saber vertex lies on the
    // x = 0 column, so it has no area: keeping only those two strips is the same
    // as drawing every reone segment.
    assert_eq!(used.into_iter().collect::<Vec<u16>>(), (0..8).chain(88..96).collect::<Vec<u16>>());
    assert_eq!(saber.saber_verts.len(), 176);
    for (i, p) in saber.saber_verts.iter().enumerate() {
        if !(0..8).contains(&i) && !(88..96).contains(&i) {
            assert!(p[0].abs() < 1e-6, "saber vertex {i} is off the x = 0 column: {p:?}");
        }
    }
    // The two halves are on opposite sides of the blade: piece 0 on +x, piece 88 on -x.
    let max_x = saber.saber_verts[..8].iter().map(|p| p[0]).fold(f32::MIN, f32::max);
    let min_x = saber.saber_verts[88..96].iter().map(|p| p[0]).fold(f32::MAX, f32::min);
    assert!(max_x > 0.09 && min_x < -0.09, "pieces 0 and 88 are the two halves: x {min_x} .. {max_x}");
}

/// The Saber nodes of a model, in pre-order.
fn saber_nodes(n: &MdlNode) -> Vec<&MdlNode> {
    let own = matches!(n.node_data, MdlNodeData::Saber(_)).then_some(n);
    own.into_iter().chain(n.children.iter().flat_map(saber_nodes)).collect()
}

#[test]
fn the_two_saber_nodes_cross_at_the_hilt() {
    let Some(model) = install_model("w_lghtsbr_001") else { return };
    let sabers = saber_nodes(&model.root_node);
    assert_eq!(sabers.len(), 2, "two Saber nodes");
    for s in &sabers {
        let p = s.position;
        assert!(p[0].abs() < 1e-4 && p[1].abs() < 1e-4 && (p[2] - 0.18).abs() < 1e-4, "{}: at {p:?}", s.name);
    }
    // Local orientations are (w, x, y, z): one is unturned, the other turned 90 degrees about z.
    let turns: Vec<[f32; 4]> = sabers.iter().map(|s| s.rotation).collect();
    let close = |a: [f32; 4], b: [f32; 4]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-3);
    let quarter = [std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2];
    assert!(turns.iter().any(|&q| close(q, [1.0, 0.0, 0.0, 0.0])), "one Saber node is unturned: {turns:?}");
    assert!(turns.iter().any(|&q| close(q, quarter)), "the other is turned 90 degrees about z: {turns:?}");
}

#[test]
fn mips_stay_inside_frame_cells() {
    // 8x8 sheet of 2x2 cells (4x4 texels each): only levels 0..=2 exist,
    // and a level-2 texel is one cell's average.
    let mut d = tpc::Decoded { width: 8, height: 8, mips: 1, data: vec![0; 8 * 8 * 4], has_alpha: false, txi: String::new() };
    for y in 0..8 {
        for x in 0..8 {
            let v = if x < 4 { 200 } else { 40 };
            d.data[(y * 8 + x) * 4..(y * 8 + x) * 4 + 4].copy_from_slice(&[v, v, v, 255]);
        }
    }
    build_mips(&mut d, (4, 4));
    assert_eq!(d.mips, 3);
    assert_eq!(d.data.len(), (64 + 16 + 4) * 4);
    let l2 = &d.data[(64 + 16) * 4..];
    assert_eq!((l2[0], l2[4]), (200, 40));
}

#[test]
fn alpha_fade_is_linear_between_keys() {
    use crate::anim::clip::Track;
    // close2open on a cage sheet: 0.35 to 0 over one second.
    let t = Track { alpha: vec![(0.0, 0.35), (1.0, 0.0)], ..Default::default() };
    assert!((t.sample_alpha(0.5).unwrap() - 0.175).abs() < 1e-6);
    assert_eq!(t.sample_alpha(2.0), Some(0.0));
}

#[test]
fn procedure_survives_junk_before_first_key() {
    let txi = "\u{1}\u{2}proceduretype cycle\nnumx 2\nnumy 2\nfps 16\nblending additive\n";
    assert_eq!(parse_procedure(txi), TexProc::Cycle { cols: 2.0, rows: 2.0, fps: 16.0 });
    assert_eq!(parse_procedure("blending additive\nproceduretype water\nspeed 4\ndistortionamplitude 0.2\n"), TexProc::Water { amplitude: 0.2, speed: 4.0 });
}

/// Loads a model from the game install (K2_INSTALL, else the install
/// around this repo). None, with a SKIP line, when K2_INSTALL is unset and no
/// install is found there; a K2_INSTALL that is not an install, a missing
/// model or one that does not parse fails the test.
fn install_model(name: &str) -> Option<k2_mdl::Mdl> {
    let set = std::env::var("K2_INSTALL").ok();
    let dir = set.clone().unwrap_or_else(|| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(&dir) else {
        assert!(set.is_none(), "K2_INSTALL={dir} is not a KotOR 2 install");
        let _ = writeln!(std::io::stderr(), "SKIP {name}: no install at {dir} (set K2_INSTALL)");
        return None;
    };
    let res = Resources::open(install).expect("the install opens");
    let mdl = res.get(&ResKey::new(name, ResourceType::Mdl)).expect("the model lookup").expect("the model is in the install");
    let mdx = res.get(&ResKey::new(name, ResourceType::Mdx)).expect("the mdx lookup");
    Some(k2_mdl::read_mdl_from_bytes(&mdl, mdx.as_deref()).expect("the model parses"))
}

/// Mesh nodes a model draws, in pre-order: (name, skinned, casts shadow).
/// Casting is the MDL trimesh flag that `TNode::casts_shadow` reads.
fn drawn_meshes(m: &k2_mdl::Mdl) -> Vec<(String, bool, bool)> {
    fn walk(n: &MdlNode, out: &mut Vec<(String, bool, bool)>) {
        if let Some(mesh) = mesh_of(n).filter(|m| m.render && !m.faces.is_empty() && !m.positions.is_empty()) {
            out.push((n.name.clone(), matches!(n.node_data, MdlNodeData::Skin(_)), mesh.shadow));
        }
        n.children.iter().for_each(|c| walk(c, out));
    }
    let mut out = Vec::new();
    walk(&m.root_node, &mut out);
    out
}

fn casters(m: &k2_mdl::Mdl) -> Vec<String> {
    drawn_meshes(m).into_iter().filter(|d| d.2).map(|d| d.0).collect()
}

fn find_node<'a>(n: &'a MdlNode, name: &str) -> Option<&'a MdlNode> {
    if n.name == name {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_node(c, name))
}

/// Pins the CURRENT caster rule for Mandalore (appearance 462,
/// P_MandaloreBB): the MDL shadow flag is honoured on skins, so only the
/// back tubes cast. This is not checked against the original; it waits on
/// the user's in-game look. Of the skin nodes in the install, 81.5% (1152
/// of 1414) have the flag set and the flag varies per skin node, so the
/// alternative is casting from every rendered skin (one line at
/// `casts_shadow` in `node()`). A caster must carry joint weights to
/// follow the pose in the shadow pass.
#[test]
fn mandalore_pins_current_caster_rule() {
    let Some(m) = install_model("p_mandalorebb") else { return };
    let drawn = drawn_meshes(&m);
    assert_eq!(drawn.iter().map(|d| d.0.as_str()).collect::<Vec<_>>(), ["torso", "larm", "rarm", "head", "tubes"]);
    assert!(drawn.iter().all(|d| d.1), "every drawn part is a skin");
    assert_eq!(casters(&m), ["tubes"]);
    let tubes = find_node(&m.root_node, "tubes").unwrap();
    let MdlNodeData::Skin(s) = &tubes.node_data else { panic!("tubes is not a skin") };
    let mut mesh = build_mesh(&s.mesh, false);
    assert!(add_skin(&mut mesh, s).is_some(), "tubes: skin without bone data");
    assert!(mesh.attribute(Mesh::ATTRIBUTE_JOINT_INDEX).is_some(), "tubes: no joint indices");
}

/// Pins the CURRENT caster lists quoted in docs/notes/rendering.md (the
/// MDL flag honoured on skins). Not checked against the original.
#[test]
fn companion_pins_current_caster_lists() {
    // Atton's clothing overlay is clear everywhere; the pmbam body under
    // it casts.
    let Some(atton) = install_model("p_attonbb") else { return };
    assert!(casters(&atton).is_empty());
    let Some(body) = install_model("pmbam") else { return };
    assert_eq!(casters(&body), ["Torso", "LArm", "RArm"]);
    // Kreia's torso and arms do not cast; her head, hands and teeth do.
    let Some(kreia) = install_model("p_kreiabb") else { return };
    assert_eq!(casters(&kreia), ["teethlwr", "teethupr", "LHand_Geo", "RHand_Geo", "Head"]);
}

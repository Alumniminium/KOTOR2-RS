//! MDL to Bevy: per-model templates (mesh + material handles per node, node
//! transforms in game space), cached by model name, spawned as entity trees.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::asset::RenderAssetUsages;
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::mesh::skinning::{SkinnedMesh, SkinnedMeshInverseBindposes};
use bevy::mesh::{Indices, PrimitiveTopology, VertexAttributeValues};
use bevy::prelude::*;
use bevy::render::render_resource::{Extent3d, TextureDimension, TextureFormat};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::light::NotShadowCaster;
use k2_mdl::{MdlControllerType, MdlFace, MdlMesh, MdlNode, MdlNodeData, MdlSaber, MdlSkin};
use k2_res::{ResKey, Resources};
use rakata_core::ResourceType;

use crate::anim::clip::{self, Clip};
use crate::material::{Blend, FLAG_ADDITIVE, FLAG_ALPHA_MASK, FLAG_LIGHTMAP, FLAG_UV_CYCLE, FLAG_UV_SCROLL, FLAG_UV_WATER, KMaterial, KParams};
use crate::tpc;

/// Access to the install. Held as a resource for the lifetime of the app.
#[derive(Resource)]
pub struct GameRes(pub Resources);

#[derive(Clone)]
pub struct TexInfo {
    pub image: Handle<Image>,
    /// Alpha is real transparency (not an env-map or bump mask).
    pub transparent: bool,
    pub additive: bool,
    /// TXI `blending punchthrough`: alpha is a cutout (alpha test), not a
    /// blend. Head textures use it; blending them lets the mouth interior
    /// show through the face, because blended meshes skip depth writes.
    pub punchthrough: bool,
    /// TXI `proceduretype` animation of the texture itself.
    pub procedure: TexProc,
}

/// TXI procedural texture kinds the renderer animates in the shader.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum TexProc {
    #[default]
    None,
    /// `proceduretype cycle`: a numx by numy grid of frames at fps.
    Cycle { cols: f32, rows: f32, fps: f32 },
    /// `proceduretype water`: distorting ripples (shimmer approximation).
    Water { amplitude: f32, speed: f32 },
}

/// Parses `proceduretype` and its parameters from lowercased TXI text.
fn parse_procedure(txi: &str) -> TexProc {
    // The first TXI line can carry stray bytes from the TPC payload in front
    // of its key (PLC_FrcDist02 starts with "proceduretype cycle"), so keys
    // are matched by suffix of the line's first token.
    let value = |key: &str| -> Option<f32> {
        txi.lines().find_map(|l| {
            let mut it = l.split_whitespace();
            it.next().is_some_and(|k| k.ends_with(key)).then(|| it.next().and_then(|v| v.parse().ok())).flatten()
        })
    };
    let kind = txi.lines().find_map(|l| {
        let mut it = l.split_whitespace();
        it.next().is_some_and(|k| k.ends_with("proceduretype")).then(|| it.next().map(str::to_owned)).flatten()
    });
    match kind.as_deref() {
        Some("cycle") => TexProc::Cycle { cols: value("numx").unwrap_or(1.0), rows: value("numy").unwrap_or(1.0), fps: value("fps").unwrap_or(10.0) },
        Some("water") => TexProc::Water { amplitude: value("distortionamplitude").unwrap_or(0.1), speed: value("speed").unwrap_or(1.0) },
        _ => TexProc::None,
    }
}

/// Appends box-filtered mip levels to a single-level RGBA8 texture, stopping
/// when a level is smaller than one texel of a `cell` sized frame so that
/// levels never mix neighbouring cells of a frame grid.
fn build_mips(d: &mut tpc::Decoded, cell: (u32, u32)) {
    let (mut w, mut h) = (d.width as usize, d.height as usize);
    let levels = (cell.0.min(cell.1).max(1).ilog2() + 1).min(w.min(h).max(1).ilog2() + 1);
    let mut data = d.data[..w * h * 4].to_vec();
    let mut prev = (0, w, h);
    for _ in 1..levels {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let (o, pw, ph) = prev;
        let mut next = vec![0u8; nw * nh * 4];
        for y in 0..nh {
            for x in 0..nw {
                for c in 0..4 {
                    let mut sum = 0u32;
                    for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                        let (sx, sy) = ((2 * x + dx).min(pw - 1), (2 * y + dy).min(ph - 1));
                        sum += u32::from(data[o + (sy * pw + sx) * 4 + c]);
                    }
                    next[(y * nw + x) * 4 + c] = ((sum + 2) / 4) as u8;
                }
            }
        }
        prev = (data.len(), nw, nh);
        data.extend_from_slice(&next);
        (w, h) = (nw, nh);
    }
    d.data = data;
    d.mips = levels;
}

/// Mesh `animate_uv` scroll: UV units per second and jitter (amount, speed).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct UvScroll {
    pub dir: [f32; 2],
    pub jitter: [f32; 2],
}

impl UvScroll {
    fn of(m: &MdlMesh) -> Option<Self> {
        (m.animate_uv != 0).then_some(Self { dir: [m.uv_direction_x, m.uv_direction_y], jitter: [m.uv_jitter, m.uv_jitter_speed] })
            .filter(|s| s.dir.iter().chain(&s.jitter).all(|v| v.is_finite() && v.abs() < 100.0))
    }

    fn key(self) -> [u32; 4] {
        [self.dir[0].to_bits(), self.dir[1].to_bits(), self.jitter[0].to_bits(), self.jitter[1].to_bits()]
    }
}

pub struct TNode {
    pub name: String,
    pub translation: Vec3,
    pub rotation: Quat,
    pub meshes: Vec<(Handle<Mesh>, Handle<KMaterial>)>,
    /// K2 trimesh flag: not drawn when the model is shown as a hologram.
    pub hide_in_holo: bool,
    /// Set when this node's mesh is skinned.
    pub skin: Option<TSkin>,
    /// MDL trimesh `shadow` flag: the mesh casts dynamic shadows.
    pub casts_shadow: bool,
    /// Light node radius. Area lights aim the shadow light (shadows.rs).
    pub light: Option<f32>,
    /// Particle emitter nodes (run for `FxLive` models only).
    pub emitter: Option<Arc<crate::emitter::EmitterSpec>>,
    pub children: Vec<TNode>,
}

/// Skinning data for a node's mesh. Joints are pre-order (DFS) node indices
/// of the same model, one per bone slot used by the vertex joint indices.
pub struct TSkin {
    pub joints: Vec<usize>,
    pub inverse_bindposes: Vec<Mat4>,
    /// Created on first spawn, shared by every instance.
    ibp_handle: Arc<std::sync::OnceLock<Handle<SkinnedMeshInverseBindposes>>>,
}

pub struct ModelTemplate {
    pub root: TNode,
    /// Lowercased supermodel name, None for "NULL".
    pub supermodel: Option<String>,
    pub anim_scale: f32,
    /// Lowercased animation root node (heads use "neck_g").
    pub anim_root: Option<String>,
    /// Animations defined in this model (not its supermodels), by lowercased name.
    pub clips: HashMap<String, Arc<Clip>>,
}

impl ModelTemplate {
    /// Nodes in pre-order, matching the entity order of `spawn_tree`.
    pub fn flat(&self) -> Vec<&TNode> {
        fn go<'a>(n: &'a TNode, out: &mut Vec<&'a TNode>) {
            out.push(n);
            for c in &n.children {
                go(c, out);
            }
        }
        let mut out = Vec::new();
        go(&self.root, &mut out);
        out
    }
}

#[derive(Resource, Default)]
pub struct AssetCache {
    textures: HashMap<String, Option<TexInfo>>,
    models: HashMap<String, Option<Arc<ModelTemplate>>>,
    materials: HashMap<(String, String, u32, [u32; 4]), Handle<KMaterial>>,
    white: Option<Handle<Image>>,
    magenta: Option<Handle<Image>>,
}

impl AssetCache {
    /// Handles of every texture loaded so far (graphics.rs re-samples them).
    pub fn texture_images(&self) -> Vec<Handle<Image>> {
        self.textures.values().flatten().map(|t| t.image.clone()).collect()
    }
}

/// Bundles the asset stores the loaders need.
pub struct Loader<'a> {
    pub res: &'a Resources,
    pub cache: &'a mut AssetCache,
    pub images: &'a mut Assets<Image>,
    pub meshes: &'a mut Assets<Mesh>,
    pub materials: &'a mut Assets<KMaterial>,
}

fn solid(images: &mut Assets<Image>, rgba: [u8; 4]) -> Handle<Image> {
    images.add(Image::new_fill(
        Extent3d { width: 1, height: 1, depth_or_array_layers: 1 },
        TextureDimension::D2,
        &rgba,
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    ))
}

fn usable_name(n: &str) -> Option<String> {
    let n = n.trim().to_ascii_lowercase();
    (!n.is_empty() && n != "null").then_some(n)
}

impl Loader<'_> {
    fn white(&mut self) -> Handle<Image> {
        if self.cache.white.is_none() {
            self.cache.white = Some(solid(self.images, [255, 255, 255, 255]));
        }
        self.cache.white.clone().unwrap()
    }

    fn magenta(&mut self) -> Handle<Image> {
        if self.cache.magenta.is_none() {
            self.cache.magenta = Some(solid(self.images, [255, 0, 255, 255]));
        }
        self.cache.magenta.clone().unwrap()
    }

    pub fn texture(&mut self, name: &str) -> Option<TexInfo> {
        if let Some(t) = self.cache.textures.get(name) {
            return t.clone();
        }
        let decoded = self
            .res
            .get(&ResKey::new(name, ResourceType::Tpc))
            .ok()
            .flatten()
            .and_then(|b| tpc::decode_tpc(&b))
            .or_else(|| self.res.get(&ResKey::new(name, ResourceType::Tga)).ok().flatten().and_then(|b| tpc::decode_tga(&b)));
        let info = decoded.map(|mut d| {
            // A sidecar .txi overrides the embedded one.
            let txi = self
                .res
                .get(&ResKey::new(name, ResourceType::Txi))
                .ok()
                .flatten()
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or(d.txi.clone())
                .to_ascii_lowercase();
            let alpha_is_mask = txi.contains("envmaptexture") || txi.contains("bumpyshinytexture") || txi.contains("bumpmaptexture");
            let blending = |mode: &str| txi.lines().any(|l| l.trim().starts_with("blending") && l.contains(mode));
            let additive = blending("additive");
            let punchthrough = blending("punchthrough");
            let procedure = parse_procedure(&txi);
            if procedure != TexProc::None && d.mips <= 1 {
                // The cage sheets ship with one level; without mips the
                // glitter aliases and shimmers as it scrolls.
                let cell = match procedure {
                    TexProc::Cycle { cols, rows, .. } => ((d.width / cols.max(1.0) as u32).max(1), (d.height / rows.max(1.0) as u32).max(1)),
                    _ => (d.width, d.height),
                };
                build_mips(&mut d, cell);
            }
            if procedure == TexProc::None {
                crate::graphics::apply_texture_quality(&mut d);
            }
            TexInfo { image: self.images.add(make_image(&d)), transparent: d.has_alpha && !alpha_is_mask, additive, punchthrough, procedure }
        });
        self.cache.textures.insert(name.to_owned(), info.clone());
        info
    }

    fn material(&mut self, mesh: &MdlMesh, lightmapped: bool) -> Handle<KMaterial> {
        let tex0 = usable_name(&mesh.texture_0).unwrap_or_default();
        let tex1 = if lightmapped { usable_name(&mesh.texture_1).unwrap_or_default() } else { String::new() };
        let scroll = UvScroll::of(mesh).filter(|_| !tex0.is_empty());
        let key = (tex0.clone(), tex1.clone(), u32::from(lightmapped), scroll.map_or([0; 4], UvScroll::key));
        if let Some(h) = self.cache.materials.get(&key) {
            return h.clone();
        }
        let mut flags = 0;
        let mut alpha_cut = 0.02;
        let mut blend = Blend::Opaque;
        let mut tint = LinearRgba::WHITE;
        let mut procedure = TexProc::None;
        let diffuse = if tex0.is_empty() {
            let [r, g, b] = mesh.diffuse_color;
            tint = LinearRgba::rgb(r, g, b);
            self.white()
        } else {
            match self.texture(&tex0) {
                Some(t) => {
                    // Transparent alpha blends; the alpha-test cut still
                    // discards fully clear texels so sorting matters less.
                    if t.additive {
                        blend = Blend::Additive;
                        flags |= FLAG_ADDITIVE;
                    } else if t.punchthrough {
                        // Opaque with an alpha-test cut, so depth is written.
                        flags |= FLAG_ALPHA_MASK;
                        alpha_cut = 0.5;
                    } else if t.transparent {
                        blend = Blend::Alpha;
                        flags |= FLAG_ALPHA_MASK;
                    }
                    procedure = t.procedure;
                    t.image
                }
                None => self.magenta(),
            }
        };
        let lightmap = if tex1.is_empty() {
            self.white()
        } else {
            match self.texture(&tex1) {
                Some(t) => {
                    flags |= FLAG_LIGHTMAP;
                    t.image
                }
                None => self.white(),
            }
        };
        let mut params = KParams::new(flags, alpha_cut, tint);
        if let Some(s) = scroll {
            params.flags |= FLAG_UV_SCROLL;
            params.scroll = Vec4::new(s.dir[0], s.dir[1], s.jitter[0], s.jitter[1]);
        }
        match procedure {
            TexProc::Cycle { cols, rows, fps } => {
                params.flags |= FLAG_UV_CYCLE;
                params.cycle = Vec4::new(cols, rows, fps, 0.0);
            }
            TexProc::Water { amplitude, speed } => {
                params.flags |= FLAG_UV_WATER;
                params.water = Vec4::new(amplitude, speed, 0.0, 0.0);
            }
            TexProc::None => {}
        }
        if params.flags & (FLAG_UV_SCROLL | FLAG_UV_CYCLE | FLAG_UV_WATER) != 0 && std::env::var_os("K2_UVDBG").is_some() {
            for t in [0.0, 0.07, 0.13, 1.0, 2.0] {
                info!("uvdbg {tex0}: {}", params.describe_at(t));
            }
        }
        let h = self.materials.add(KMaterial {
            params,
            diffuse,
            lightmap,
            blend,
        });
        self.cache.materials.insert(key, h.clone());
        h
    }

    pub fn model(&mut self, name: &str) -> Option<Arc<ModelTemplate>> {
        let name = name.to_ascii_lowercase();
        if let Some(m) = self.cache.models.get(&name) {
            return m.clone();
        }
        let mdl = self.res.get(&ResKey::new(&name, ResourceType::Mdl)).ok().flatten();
        let mdx = self.res.get(&ResKey::new(&name, ResourceType::Mdx)).ok().flatten();
        let parsed = mdl.and_then(|b| match k2_mdl::read_mdl_from_bytes(&b, mdx.as_deref()) {
            Ok(m) => Some(m),
            Err(e) => {
                warn!("model {name}: {e}");
                None
            }
        });
        let t = parsed.map(|m| {
            let mut counter = 0;
            let root = self.node(&m.root_node, &mut counter);
            let sup = m.supermodel_name.trim().to_ascii_lowercase();
            Arc::new(ModelTemplate {
                root,
                supermodel: (!sup.is_empty() && sup != "null" && sup != name).then_some(sup),
                anim_scale: if m.animation_scale > 0.0 { m.animation_scale } else { 1.0 },
                anim_root: m.anim_root_node.as_ref().map(|s| s.to_ascii_lowercase()),
                clips: clip::convert_all(&m.animations),
            })
        });
        if let Some(t) = &t {
            fn count(n: &TNode) -> usize { n.meshes.len() + n.children.iter().map(count).sum::<usize>() }
            debug!("model {name}: {} meshes", count(&t.root));
        }
        if t.is_none() {
            warn!("model {name} missing");
        }
        self.cache.models.insert(name, t.clone());
        t
    }

    /// Material for an explicit diffuse texture, unlit by lightmaps. Used for
    /// creature texture variants.
    pub fn texture_material(&mut self, tex: &str) -> Handle<KMaterial> {
        let mesh = MdlMesh { texture_0: tex.to_owned(), ..Default::default() };
        self.material(&mesh, false)
    }

    fn node(&mut self, n: &MdlNode, counter: &mut usize) -> TNode {
        *counter += 1;
        let [w, x, y, z] = n.rotation;
        let rot = Quat::from_xyzw(x, y, z, w);
        let rotation = if rot.length_squared() > 1e-6 { rot.normalize() } else { Quat::IDENTITY };
        let mut meshes = Vec::new();
        // A lightsaber's blade is the saber node's own mesh (the glow sheets are plain meshes).
        let blade = match &n.node_data {
            MdlNodeData::Saber(s) => Some(saber_blade(s)),
            _ => None,
        };
        let renderable = blade.as_ref().or_else(|| mesh_of(n));
        let mut skin = None;
        let casts_shadow = renderable.is_none_or(|m| m.shadow);
        let light = match &n.node_data {
            MdlNodeData::Light(_) => {
                Some(n.controllers.iter().find(|c| c.controller_type == MdlControllerType::RADIUS).and_then(|c| c.keys.first()).and_then(|k| k.values.first()).copied().unwrap_or(5.0))
            }
            _ => None,
        };
        let hide_in_holo = renderable.is_some_and(|m| m.tsl.is_some_and(|t| t.hide_in_holograms));
        // Door models carry an untextured `trans` quad filling the doorway
        // (transition marker); the engine never draws it.
        let marker = n.name.eq_ignore_ascii_case("trans");
        if let Some(mesh) = renderable.filter(|m| !marker && m.render && !m.faces.is_empty() && !m.positions.is_empty()) {
            let lightmapped = mesh.light_mapped && mesh.uv2.len() == mesh.positions.len();
            let mat = self.material(mesh, lightmapped);
            if std::env::var_os("K2_UVDBG").is_some() {
                if let Some(m) = self.materials.get(&mat) {
                    let p = &m.params;
                    info!(
                        "uvnode {}: tex {} blend {:?} flags {:#x} animate_uv {} dir ({}, {}) scroll_uv/s ({:.3}, {:.3}) cycle {:?} water {:?} pos {:?} bbox {:?}..{:?}",
                        n.name, mesh.texture_0.trim(), m.blend, p.flags, mesh.animate_uv, mesh.uv_direction_x, mesh.uv_direction_y, p.scroll.x, p.scroll.y, p.cycle, p.water, n.position, mesh.bounding_min, mesh.bounding_max
                    );
                }
            }
            let mut built = build_mesh(mesh, lightmapped);
            if let MdlNodeData::Skin(s) = &n.node_data {
                skin = add_skin(&mut built, s);
            }
            meshes.push((self.meshes.add(built), mat));
        }
        TNode {
            name: n.name.clone(),
            translation: Vec3::from_array(n.position),
            rotation,
            meshes,
            hide_in_holo,
            skin,
            casts_shadow,
            light,
            emitter: crate::emitter::EmitterSpec::from_node(n),
            children: n.children.iter().map(|c| self.node(c, counter)).collect(),
        }
    }
}

/// The blade strip inside an 8-vertex piece: two rows of four (0-3 and 4-7)
/// and three quads between them. This is reone's kSaberPieceFaceIndices
/// (seedhartha/reone, GPL-3), checked against the w_lghtsbr models: its winding
/// gives the saber normals.
const BLADE_STRIP: [[u16; 3]; 6] = [[4, 5, 0], [5, 1, 0], [5, 6, 1], [6, 2, 1], [6, 7, 2], [7, 3, 2]];

/// A lightsaber blade as a mesh of its own. Its positions, UVs and normals are
/// the Saber node's saber_verts, saber_uvs and saber_normals (176 each). The
/// base mesh's 12 faces (over vertices 0-33, mostly slivers in the bind pose)
/// are replaced.
///
/// In the bind pose only the pieces 0-7 (on +x) and 88-95 (on -x) have area.
/// 88 is half of the 176 saber verts: the first piece of the second side (reone
/// does not name the offset; the data does). The other pieces repeat the x = 0
/// column, so they get no triangles. The piece at 88 is wound the other way, as
/// the saber normals need. The two strips make a flat blade about 0.2 m wide and
/// 1 m long; the second Saber node is the same mesh turned 90 degrees about the
/// blade.
fn saber_blade(s: &MdlSaber) -> MdlMesh {
    let mut m = s.mesh.clone();
    m.positions = s.saber_verts.clone();
    m.uv1 = s.saber_uvs.clone();
    m.normals = s.saber_normals.clone();
    let front = BLADE_STRIP.iter().copied();
    let back = BLADE_STRIP.iter().map(|&[a, b, c]| [88 + c, 88 + b, 88 + a]);
    m.faces = front.chain(back).map(|vertex_indices| MdlFace { vertex_indices, ..MdlFace::default() }).collect();
    m
}

/// The triangle mesh behind a node (plain, skinned, dangly or animated).
fn mesh_of(n: &MdlNode) -> Option<&MdlMesh> {
    match &n.node_data {
        MdlNodeData::Mesh(m) => Some(m),
        MdlNodeData::Skin(s) => Some(&s.mesh),
        MdlNodeData::Dangly(d) => Some(&d.mesh),
        MdlNodeData::AnimMesh(a) => Some(&a.mesh),
        _ => None,
    }
}

fn build_mesh(m: &MdlMesh, lightmapped: bool) -> Mesh {
    let n = m.positions.len();
    // Keep a main-world copy: the follow camera ray-casts room and
    // placeable meshes (play.rs), which needs the vertex data on the CPU.
    let mut mesh = Mesh::new(PrimitiveTopology::TriangleList, RenderAssetUsages::default());
    mesh.insert_attribute(Mesh::ATTRIBUTE_POSITION, m.positions.clone());
    let has_normals = m.normals.len() == n;
    if has_normals {
        mesh.insert_attribute(Mesh::ATTRIBUTE_NORMAL, m.normals.clone());
    }
    if m.uv1.len() == n {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_0, m.uv1.clone());
    }
    if lightmapped {
        mesh.insert_attribute(Mesh::ATTRIBUTE_UV_1, m.uv2.clone());
    }
    let idx: Vec<u32> = m
        .faces
        .iter()
        .flat_map(|f| f.vertex_indices)
        .map(u32::from)
        .collect();
    mesh.insert_indices(Indices::U32(idx));
    if !has_normals {
        mesh.compute_flat_normals();
    }
    mesh
}

/// Adds joint attributes to a skin mesh and returns its joint setup.
///
/// The skin's bonemap is indexed by pre-order node index and holds the bone
/// slot (as f32 bits; -1 = not a bone). qbone/tbone are per pre-order node:
/// the transform from the skin node's space into that bone's space, i.e. the
/// inverse bind pose relative to the skin node.
fn add_skin(mesh: &mut Mesh, s: &MdlSkin) -> Option<TSkin> {
    let n = s.mesh.positions.len();
    if s.bone_weights.len() != n || s.bone_indices.len() != n {
        return None;
    }
    let slots: Vec<i32> = s.bonemap.iter().map(|v| f32::from_bits(*v) as i32).collect();
    let count = slots.iter().copied().max().unwrap_or(-1) + 1;
    if count <= 0 {
        return None;
    }
    let mut joints = vec![0usize; count as usize];
    let mut ibp = vec![Mat4::IDENTITY; count as usize];
    for (node, &slot) in slots.iter().enumerate() {
        if slot < 0 {
            continue;
        }
        joints[slot as usize] = node;
        let q = s.qbone_ref_inv.get(node).map(|[w, x, y, z]| Quat::from_xyzw(*x, *y, *z, *w)).unwrap_or(Quat::IDENTITY);
        let q = if q.length_squared() > 1e-8 { q.normalize() } else { Quat::IDENTITY };
        let t = s.tbone_ref_inv.get(node).map(|v| Vec3::from_array(*v)).unwrap_or(Vec3::ZERO);
        ibp[slot as usize] = Mat4::from_rotation_translation(q, t);
    }
    let mut idx = Vec::with_capacity(n);
    let mut wts = Vec::with_capacity(n);
    for (bi, bw) in s.bone_indices.iter().zip(&s.bone_weights) {
        let mut i = [0u16; 4];
        let mut w = [0f32; 4];
        for k in 0..4 {
            let b = bi[k] as i32;
            if b >= 0 && b < count && bw[k] > 0.0 {
                i[k] = b as u16;
                w[k] = bw[k];
            }
        }
        let sum: f32 = w.iter().sum();
        if sum > 1e-6 {
            w.iter_mut().for_each(|v| *v /= sum);
        } else {
            w = [1.0, 0.0, 0.0, 0.0];
        }
        idx.push(i);
        wts.push(w);
    }
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_INDEX, VertexAttributeValues::Uint16x4(idx));
    mesh.insert_attribute(Mesh::ATTRIBUTE_JOINT_WEIGHT, wts);
    Some(TSkin { joints, inverse_bindposes: ibp, ibp_handle: Default::default() })
}

fn make_image(d: &tpc::Decoded) -> Image {
    let mut img = Image::new(
        Extent3d { width: d.width, height: d.height, depth_or_array_layers: 1 },
        TextureDimension::D2,
        d.data.clone(),
        TextureFormat::Rgba8UnormSrgb,
        RenderAssetUsages::RENDER_WORLD,
    );
    img.texture_descriptor.mip_level_count = d.mips;
    img.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
        address_mode_u: ImageAddressMode::Repeat,
        address_mode_v: ImageAddressMode::Repeat,
        mag_filter: ImageFilterMode::Linear,
        min_filter: ImageFilterMode::Linear,
        mipmap_filter: ImageFilterMode::Linear,
        anisotropy_clamp: crate::graphics::anisotropy(),
        ..default()
    });
    img
}

/// Spawns a template as an entity tree under `parent`. Returns the root.
pub fn spawn_template(commands: &mut Commands, t: &ModelTemplate, parent: Entity, transform: Transform) -> Entity {
    spawn_tree(commands, t, parent, transform, None)[0]
}

/// Spawns a template and returns its node entities in pre-order (index 0 is
/// the root). `material` replaces every mesh's material when given.
pub fn spawn_tree(
    commands: &mut Commands,
    t: &ModelTemplate,
    parent: Entity,
    transform: Transform,
    material: Option<&Handle<KMaterial>>,
) -> Vec<Entity> {
    let mut nodes = Vec::new();
    let mut skinned = Vec::new();
    spawn_node(commands, &t.root, parent, material, &mut nodes, &mut skinned);
    commands.entity(nodes[0]).insert(transform * Transform::from_translation(t.root.translation).with_rotation(t.root.rotation));
    let flat = t.flat();
    for (node_idx, mesh_entities) in skinned {
        let Some(skin) = flat[node_idx].skin.as_ref() else { continue };
        let joints: Vec<Entity> = skin.joints.iter().map(|&j| nodes.get(j).copied().unwrap_or(nodes[0])).collect();
        let ibp = skin.inverse_bindposes.clone();
        let cell = skin.ibp_handle.clone();
        commands.queue(move |world: &mut World| {
            let handle = cell
                .get_or_init(|| world.resource_mut::<Assets<SkinnedMeshInverseBindposes>>().add(SkinnedMeshInverseBindposes::from(ibp)))
                .clone();
            for e in mesh_entities {
                if let Ok(mut em) = world.get_entity_mut(e) {
                    em.insert((SkinnedMesh { inverse_bindposes: handle.clone(), joints: joints.clone() }, NoFrustumCulling));
                }
            }
        });
    }
    nodes
}

fn spawn_node(
    commands: &mut Commands,
    n: &TNode,
    parent: Entity,
    material: Option<&Handle<KMaterial>>,
    nodes: &mut Vec<Entity>,
    skinned: &mut Vec<(usize, Vec<Entity>)>,
) {
    let e = commands
        .spawn((Name::new(n.name.clone()), Transform::from_translation(n.translation).with_rotation(n.rotation), Visibility::default()))
        .id();
    let index = nodes.len();
    nodes.push(e);
    commands.entity(parent).add_child(e);
    if let Some(spec) = &n.emitter {
        commands.entity(e).insert(crate::emitter::EmitterNode::new(spec.clone()));
    }
    if let Some(radius) = n.light {
        commands.entity(e).insert(crate::shadows::ModelLight { radius });
    }
    let mut mesh_entities = Vec::new();
    for (mesh, mat) in &n.meshes {
        let mat = material.cloned().unwrap_or_else(|| mat.clone());
        let m = commands.spawn((Mesh3d(mesh.clone()), MeshMaterial3d(mat))).id();
        if n.hide_in_holo {
            commands.entity(m).insert(crate::holo::HideInHolo);
        }
        if !n.casts_shadow {
            commands.entity(m).insert(NotShadowCaster);
        }
        commands.entity(e).add_child(m);
        mesh_entities.push(m);
    }
    if n.skin.is_some() && !mesh_entities.is_empty() {
        skinned.push((index, mesh_entities));
    }
    for c in &n.children {
        spawn_node(commands, c, e, material, nodes, skinned);
    }
}

#[cfg(test)]
mod tests;

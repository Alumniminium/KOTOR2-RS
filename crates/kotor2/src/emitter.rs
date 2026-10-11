//! Particle emitters (MDL emitter nodes), for models marked `FxLive`.
//!
//! Only the minigame models are marked so far (swoop crash sparks, turret
//! fighter explosions). Two emitter kinds run:
//!
//! - `Explosion` emitters wait for the clip event `detonate` (the fighter's
//!   `die` clip fires one per emitter, all at 0.07 s) and then emit
//!   `birthrate` particles at once: the fireball (Fx_Explode_01, a 4x4 frame
//!   sheet played at fps), flare sparks (fx_Flare03, lighten) and debris
//!   chunks (`chunk_name`, a small model that tumbles).
//! - `Fountain` emitters emit `birthrate` particles per second, but only when
//!   a clip drives the birthrate (the `die` clip raises Smoke from 0 to 15
//!   for 0.4 s). Fountains with a constant birthrate (engine thrusters) are
//!   not drawn yet.
//!
//! Emitter controllers (codes in k2-mdl `controllers.rs`): lifeExp,
//! velocity, randvel, spread (radians, the cone around the emitter's local
//! Z; 2 pi is a full sphere), grav, drag, sizeStart/Mid/End (metres),
//! colorStart/Mid/End, alphaStart/Mid/End, percentStart/Mid/End (where in
//! the life the start/mid/end values apply), fps and frameStart/End for the
//! frame sheet, particleRot. Sizes and speeds are taken as metres; the
//! explosion fireball is 45 m across next to a 14 m fighter, which is how it
//! reads in the turret view. Particles are camera-facing quads in world
//! space (their own entities), blended additively for `Lighten` or when the
//! texture is additive.

use std::collections::HashMap;
use std::sync::Arc;

use bevy::prelude::*;
use k2_mdl::{MdlControllerType, MdlEmitter, MdlNode, MdlNodeData};

use crate::anim::AnimPlayer;
use crate::anim::clip::Clip;
use crate::material::{Blend, FLAG_ADDITIVE, FLAG_ALPHA_MASK, KMaterial};
use crate::model::{AssetCache, GameRes, Loader, spawn_tree};

pub struct EmitterPlugin;

impl Plugin for EmitterPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FxCache>().add_systems(Update, (emit, step_particles).chain());
    }
}

/// Marks a model root whose emitter nodes run.
#[derive(Component)]
pub struct FxLive;

/// Clips layered over a model's main clip (minigame overlays): (clip, time).
#[derive(Component, Default)]
pub struct FxClips(pub Vec<(Arc<Clip>, f32)>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Fountain,
    Explosion,
    Other,
}

/// What an emitter node says (controller values at their first key).
#[derive(Debug)]
pub struct EmitterSpec {
    name: String,
    kind: Kind,
    additive: bool,
    texture: String,
    grid: (u32, u32),
    chunk: Option<String>,
    birthrate: f32,
    life: f32,
    velocity: f32,
    randvel: f32,
    spread: f32,
    grav: f32,
    drag: f32,
    size: [f32; 3],
    size_y: [f32; 3],
    color: [[f32; 3]; 3],
    alpha: [f32; 3],
    percent: [f32; 3],
    rot: f32,
    fps: f32,
    frames: (u32, u32),
}

impl EmitterSpec {
    /// The spec of an emitter node, None for other node kinds.
    pub fn from_node(n: &MdlNode) -> Option<Arc<Self>> {
        let MdlNodeData::Emitter(e) = &n.node_data else { return None };
        Some(Arc::new(Self::build(n, e)))
    }

    fn build(n: &MdlNode, e: &MdlEmitter) -> Self {
        let ctl = |code: u32| n.controllers.iter().find(|c| c.controller_type == MdlControllerType::from_raw(code)).and_then(|c| c.keys.first()).map(|k| k.values.clone());
        let f = |code: u32, default: f32| ctl(code).and_then(|v| v.first().copied()).filter(|x| x.is_finite()).unwrap_or(default);
        let rgb = |code: u32| {
            let v = ctl(code).unwrap_or_default();
            if v.len() >= 3 { [v[0], v[1], v[2]] } else { [1.0; 3] }
        };
        let kind = match e.update.trim().to_ascii_lowercase().as_str() {
            "fountain" => Kind::Fountain,
            "explosion" => Kind::Explosion,
            _ => Kind::Other,
        };
        let (gx, gy) = (e.x_grid.max(1) as u32, e.y_grid.max(1) as u32);
        let total = gx * gy;
        let (f0, f1) = (f(112, 0.0).max(0.0) as u32, f(108, 0.0).max(0.0) as u32);
        let size = [f(144, 1.0), f(232, f(144, 1.0)), f(148, f(144, 1.0))];
        let size_y = [f(152, 0.0), f(236, 0.0), f(156, 0.0)];
        let chunk = Some(e.chunk_name.trim().to_owned()).filter(|c| !c.is_empty() && !c.eq_ignore_ascii_case("null"));
        Self {
            name: n.name.to_ascii_lowercase(),
            kind,
            additive: e.blend.trim().eq_ignore_ascii_case("lighten"),
            texture: e.texture.trim().to_owned(),
            grid: (gx, gy),
            chunk,
            birthrate: f(88, 0.0),
            life: f(120, 1.0).max(0.05),
            velocity: f(168, 0.0),
            randvel: f(140, 0.0),
            spread: f(160, 0.0),
            grav: f(116, 0.0),
            drag: f(100, 0.0),
            size,
            size_y: [if size_y[0] > 0.0 { size_y[0] } else { size[0] }, if size_y[1] > 0.0 { size_y[1] } else { size[1] }, if size_y[2] > 0.0 { size_y[2] } else { size[2] }],
            color: [rgb(392), rgb(284), rgb(380)],
            alpha: [f(84, 1.0), f(216, 1.0), f(80, 1.0)],
            percent: [f(220, 0.0), f(224, 0.0), f(228, 1.0)],
            rot: f(136, 0.0),
            fps: f(104, 0.0),
            frames: (f0.min(total - 1), if f1 == 0 { total - 1 } else { f1.min(total - 1) }),
        }
    }

    /// Start/mid/end interpolation by the life fraction.
    fn over_life<const N: usize>(&self, v: [[f32; N]; 3], t: f32) -> [f32; N] {
        let [_, pm, pe] = self.percent;
        let (a, b, f) = if t < pm && pm > 0.0 {
            (v[0], v[1], t / pm)
        } else {
            let span = (pe - pm).max(1e-3);
            (v[1], v[2], ((t - pm) / span).clamp(0.0, 1.0))
        };
        std::array::from_fn(|i| a[i] + (b[i] - a[i]) * f)
    }
}

/// On an emitter node entity.
#[derive(Component)]
pub struct EmitterNode {
    spec: Arc<EmitterSpec>,
    carry: f32,
    /// Last sampled time per active clip name, for event crossings.
    last: HashMap<String, f32>,
}

impl EmitterNode {
    pub fn new(spec: Arc<EmitterSpec>) -> Self {
        Self { spec, carry: 0.0, last: HashMap::new() }
    }
}

#[derive(Component)]
struct Particle {
    spec: Arc<EmitterSpec>,
    vel: Vec3,
    age: f32,
    life: f32,
    rot: f32,
    rotv: f32,
    frame: u32,
    mat: Option<Handle<KMaterial>>,
    axis: Vec3,
    chunk: bool,
}

/// Shared quads (one per sheet cell) and base materials.
#[derive(Resource, Default)]
struct FxCache {
    quads: HashMap<(u32, u32), Vec<Handle<Mesh>>>,
    mats: HashMap<(String, bool), Handle<KMaterial>>,
}

/// Small deterministic generator (particles need no quality randomness).
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn signed(&mut self) -> f32 {
        self.next() * 2.0 - 1.0
    }
}

fn quad_cells(meshes: &mut Assets<Mesh>, cols: u32, rows: u32) -> Vec<Handle<Mesh>> {
    let mut out = Vec::new();
    for r in 0..rows {
        for c in 0..cols {
            let (u0, u1) = (c as f32 / cols as f32, (c + 1) as f32 / cols as f32);
            let (v0, v1) = (r as f32 / rows as f32, (r + 1) as f32 / rows as f32);
            let mut m = Mesh::new(bevy::mesh::PrimitiveTopology::TriangleList, bevy::asset::RenderAssetUsages::default());
            m.insert_attribute(Mesh::ATTRIBUTE_POSITION, vec![[-0.5, -0.5, 0.0], [0.5, -0.5, 0.0], [0.5, 0.5, 0.0], [-0.5, 0.5, 0.0]]);
            m.insert_attribute(Mesh::ATTRIBUTE_NORMAL, vec![[0.0, 0.0, 1.0]; 4]);
            m.insert_attribute(Mesh::ATTRIBUTE_UV_0, vec![[u0, v1], [u1, v1], [u1, v0], [u0, v0]]);
            m.insert_indices(bevy::mesh::Indices::U32(vec![0, 1, 2, 0, 2, 3]));
            out.push(meshes.add(m));
        }
    }
    out
}

fn frame_of(spec: &EmitterSpec, age: f32, life: f32) -> u32 {
    let (f0, f1) = spec.frames;
    if spec.grid.0 * spec.grid.1 <= 1 {
        return 0;
    }
    let n = (f1 + 1).saturating_sub(f0).max(1);
    let k = if spec.fps > 0.0 { (age * spec.fps) as u32 } else { ((age / life) * n as f32) as u32 };
    f0 + k.min(n - 1)
}

/// Emission: bursts on `detonate`, fountains by birthrate.
#[allow(clippy::too_many_arguments)]
fn emit(
    mut commands: Commands,
    time: Res<Time>,
    game: Res<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut fx: ResMut<FxCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut emitters: Query<(Entity, &GlobalTransform, &mut EmitterNode)>,
    parents: Query<&ChildOf>,
    roots: Query<(&AnimPlayer, Option<&FxClips>), With<FxLive>>,
    mut seed: Local<u64>,
) {
    let dt = time.delta_secs();
    if dt <= 0.0 {
        return;
    }
    let mut rng = Rng(*seed ^ 0x9E3779B97F4A7C15);
    for (e, gt, mut em) in &mut emitters {
        let spec = em.spec.clone();
        if !matches!(spec.kind, Kind::Fountain | Kind::Explosion) {
            continue;
        }
        // The model root above this node.
        let mut cur = e;
        let mut root = None;
        for _ in 0..24 {
            if roots.contains(cur) {
                root = Some(cur);
                break;
            }
            match parents.get(cur) {
                Ok(p) => cur = p.parent(),
                Err(_) => break,
            }
        }
        let Some((player, overlays)) = root.and_then(|r| roots.get(r).ok()) else { continue };
        let mut clips: Vec<(Arc<Clip>, f32)> = Vec::new();
        if let Some((c, t)) = player.current_clip() {
            clips.push((c.clone(), t));
        }
        if let Some(o) = overlays {
            clips.extend(o.0.iter().cloned());
        }
        let mut rate = spec.birthrate;
        let mut driven = false;
        let mut burst = false;
        for (clip, t) in &clips {
            if let Some(tr) = clip.tracks.get(&spec.name).and_then(|tr| tr.sample_birthrate(*t)) {
                rate = tr;
                driven = true;
            }
            let prev = em.last.get(&clip.name).copied().filter(|&p| p <= *t + 1e-4).unwrap_or(-1.0);
            if clip.events.iter().any(|(et, name)| name == "detonate" && *et > prev && *et <= *t) {
                burst = true;
            }
            em.last.insert(clip.name.clone(), *t);
        }
        let count = match spec.kind {
            Kind::Explosion if burst => spec.birthrate.round().max(0.0) as u32,
            Kind::Fountain if driven && rate > 0.0 => {
                em.carry += rate * dt;
                let n = em.carry.floor();
                em.carry -= n;
                n as u32
            }
            _ => 0,
        };
        if count == 0 {
            continue;
        }
        let tf = gt.compute_transform();
        let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
        let key = (spec.texture.to_ascii_lowercase(), spec.additive);
        let mat = if spec.chunk.is_some() || spec.texture.is_empty() {
            None
        } else {
            Some(
                fx.mats
                    .entry(key)
                    .or_insert_with(|| {
                        let h = loader.texture_material(&spec.texture);
                        let mut m = loader.materials.get(&h).cloned().expect("texture_material adds its material");
                        if spec.additive || m.blend == Blend::Additive {
                            m.blend = Blend::Additive;
                            m.params.flags |= FLAG_ADDITIVE;
                        } else {
                            m.blend = Blend::Alpha;
                            m.params.flags |= FLAG_ALPHA_MASK;
                            m.params.alpha_cut = 0.02;
                        }
                        loader.materials.add(m)
                    })
                    .clone(),
            )
        };
        let quads = fx.quads.entry(spec.grid).or_insert_with(|| quad_cells(loader.meshes, spec.grid.0, spec.grid.1)).clone();
        for _ in 0..count.min(300) {
            // Direction: a cone around the emitter's local Z, a full sphere at 2 pi.
            let half = (spec.spread * 0.5).clamp(0.0, std::f32::consts::PI);
            let cos_t = 1.0 - rng.next() * (1.0 - half.cos());
            let sin_t = (1.0 - cos_t * cos_t).max(0.0).sqrt();
            let phi = rng.next() * std::f32::consts::TAU;
            let local = Vec3::new(sin_t * phi.cos(), sin_t * phi.sin(), cos_t);
            // Emitter space is game space (Z up); Bevy's world is Y up.
            let dir = tf.rotation * crate::coords::to_bevy(local);
            let speed = spec.velocity + spec.randvel * rng.signed();
            let life = spec.life * (0.8 + 0.4 * rng.next());
            let vel = dir * speed;
            let rotv = spec.rot * rng.signed();
            let mut p = Particle { spec: spec.clone(), vel, age: 0.0, life, rot: rng.next() * std::f32::consts::TAU, rotv, frame: 0, mat: None, axis: Vec3::new(rng.signed(), rng.signed(), rng.signed()).normalize_or(Vec3::Y), chunk: false };
            if let Some(chunk) = &spec.chunk {
                let Some(tpl) = loader.model(chunk) else { continue };
                p.chunk = true;
                let holder = commands.spawn((Name::new("fx chunk"), Transform::from_translation(tf.translation).with_scale(Vec3::splat(1.0)), Visibility::default())).id();
                spawn_tree(&mut commands, &tpl, holder, Transform::IDENTITY, None);
                commands.entity(holder).insert(p);
            } else if let Some(base) = &mat {
                let own = loader.materials.get(base).cloned().map(|m| loader.materials.add(m));
                p.mat = own.clone();
                let Some(own) = own else { continue };
                let q = quads[(p.frame as usize).min(quads.len() - 1)].clone();
                commands.spawn((
                    Name::new("fx particle"),
                    Mesh3d(q),
                    MeshMaterial3d(own),
                    Transform::from_translation(tf.translation).with_scale(Vec3::ZERO),
                    Visibility::default(),
                    bevy::light::NotShadowCaster,
                    p,
                ));
            }
        }
    }
    *seed = seed.wrapping_add(0x1234567);
}

/// Moves, fades and frames the live particles; they face the camera.
#[allow(clippy::too_many_arguments)]
fn step_particles(
    mut commands: Commands,
    time: Res<Time>,
    fx: Res<FxCache>,
    mut materials: ResMut<Assets<KMaterial>>,
    cams: Query<&GlobalTransform, With<Camera3d>>,
    mut ps: Query<(Entity, &mut Particle, &mut Transform, Option<&mut Mesh3d>)>,
) {
    let dt = time.delta_secs();
    let cam = cams.iter().next().map(|g| g.compute_transform().rotation).unwrap_or(Quat::IDENTITY);
    for (e, mut p, mut tf, mesh) in &mut ps {
        p.age += dt;
        if p.age >= p.life {
            commands.entity(e).despawn();
            continue;
        }
        let spec = p.spec.clone();
        let t = p.age / p.life;
        let drag = (1.0 - spec.drag * dt).clamp(0.0, 1.0);
        p.vel = p.vel * drag + Vec3::NEG_Y * spec.grav * dt;
        tf.translation += p.vel * dt;
        p.rot += p.rotv * dt;
        if p.chunk {
            tf.rotation = Quat::from_axis_angle(p.axis, p.rot);
            continue;
        }
        let [sx] = spec.over_life([[spec.size[0]], [spec.size[1]], [spec.size[2]]], t);
        let [sy] = spec.over_life([[spec.size_y[0]], [spec.size_y[1]], [spec.size_y[2]]], t);
        tf.scale = Vec3::new(sx, sy, 1.0);
        tf.rotation = cam * Quat::from_rotation_z(p.rot);
        let [r, g, b] = spec.over_life(spec.color, t);
        let [a] = spec.over_life([[spec.alpha[0]], [spec.alpha[1]], [spec.alpha[2]]], t);
        if let Some(h) = &p.mat
            && let Some(mut m) = materials.get_mut(h)
        {
            // The shader lights non-lightmapped meshes at 0.55 to 1.15;
            // lift the colour so fire is not dimmed.
            m.params.tint = LinearRgba::new(r * 1.2, g * 1.2, b * 1.2, a.clamp(0.0, 1.0));
        }
        let frame = frame_of(&spec, p.age, p.life);
        if frame != p.frame {
            p.frame = frame;
            if let (Some(mut m), Some(q)) = (mesh, fx.quads.get(&spec.grid)) {
                m.0 = q[(frame as usize).min(q.len() - 1)].clone();
            }
        }
    }
}

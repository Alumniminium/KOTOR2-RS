//! Swoop racing and turret minigames: the ARE `MiniGame` struct, the
//! simulation, and the SWMG_* engine routines.
//!
//! A minigame area has one player follower and lists of enemies (other
//! followers: mines, accelerator pads, rival swoops, fighters) and
//! obstacles. Every follower rides a *track*: a model listed in the LYT
//! `tracks` section whose `modelhook` node is animated (animation "track")
//! along the path. Obstacles are LYT `obstacles` models used as static
//! collision boxes.
//!
//! * Swoop (Type 1): the player moves along its track at `speed` units/s
//!   (scripts drive speed, acceleration and gears through SWMG_*), steers
//!   sideways within the tunnel, and jumps (SWMG_SetJumpSpeed). Touching a
//!   follower runs that follower's OnHitFollower (OBJECT_SELF = follower) and
//!   the player's OnHitFollower; touching an obstacle runs the player's
//!   OnHitObstacle and bumps the swoop back.
//! * Turret (Type 2): the player sits on a static track and the offset is
//!   the gun's rotation in degrees (x = pitch within the tunnel limits, z =
//!   yaw, wrapping when TunnelInfinite.z is set), driven by the mouse per
//!   the Mouse AxisX/AxisY mapping. Gun banks fire bullets at the targets
//!   their Target_Type names (1 = player, 2 = enemies); enemies fire back
//!   inside their sensing radius.
//!
//! The player's OnHeartbeat runs every `HEARTBEAT` seconds (race timing
//! and the turret HUD read the clock there).

use std::collections::HashSet;

use glam::{Quat, Vec2, Vec3};
use k2_data::GffExt;
use k2_script::{ObjectId, Value, OBJECT_INVALID};
use rakata_core::ResourceType;
use rakata_formats::gff::{GffStruct, GffValue};

use crate::{ScriptCtx, World};

pub const KIND_SWOOP: u32 = 1;
pub const KIND_TURRET: u32 = 2;
/// Player heartbeat period.
pub const HEARTBEAT: f32 = 0.1;
/// Bullet target types (Gun_Banks Target_Type).
pub const TARGET_PLAYER: u32 = 1;
pub const TARGET_ENEMY: u32 = 2;
/// Swoop jump gravity, units/s^2.
const GRAVITY: f32 = 45.0;
/// Seconds the swoop needs for a full tunnel crossing at peak sideways
/// speed (the scripts' lateral acceleration only decides how fast it gets
/// there).
const CROSSING_SECS: f32 = 1.1;
/// Peak sideways speed limits, units/s.
const LATERAL_MAX_SPEED: (f32, f32) = (20.0, 38.0);
/// Lateral acceleration is kept in this range (the scripts set it to the
/// current speed, 0 until the race starts: no steering before "Go").
const LATERAL_ACCEL_RANGE: (f32, f32) = (120.0, 300.0);
/// Reversing direction pushes this much harder than speeding up.
const LATERAL_REVERSE_BOOST: f32 = 2.0;
/// Sideways speed decay per second when no steering key is held.
const LATERAL_DAMPING: f32 = 9.0;
/// Minimum seconds between automatic gear shifts.
const SHIFT_COOLDOWN: f32 = 0.35;
/// Global the swoop scripts keep the gear in (-5..-1 countdown, 0 = go,
/// 1.. = gears).
const GEAR_GLOBAL: &str = "min_race_gear";

/// A path from a track model.
#[derive(Debug, Clone, Default)]
pub struct Track {
    pub name: String,
    /// LYT track position (world).
    pub origin: Vec3,
    pub pos: Vec<(f32, Vec3)>,
    pub rot: Vec<(f32, Quat)>,
    /// Animation length in seconds.
    pub length: f32,
    /// Cumulative arc length at each `pos` key.
    arc: Vec<f32>,
}

impl Track {
    pub fn load(world: &World, name: &str) -> Track {
        let origin = world
            .module
            .as_ref()
            .and_then(|m| m.layout.tracks.iter().find(|t| t.model.eq_ignore_ascii_case(name)))
            .map_or(Vec3::ZERO, |t| Vec3::from(t.position));
        let mut track = Track { name: name.to_ascii_lowercase(), origin, ..Default::default() };
        let Some(mdl) = load_mdl(world, name) else { return track };
        // Position keys are offsets from the hook's rest position (as for
        // every animated KotOR node); orientation keys are absolute.
        let (rest_p, rest_r) = find_node(&mdl.root_node, "modelhook").map_or((Vec3::ZERO, Quat::IDENTITY), |n| {
            let r = n.rotation;
            let q = Quat::from_xyzw(r[1], r[2], r[3], r[0]);
            (Vec3::from(n.position), if q.length_squared() > 1e-8 { q.normalize() } else { Quat::IDENTITY })
        });
        let anim = mdl.animations.iter().find(|a| a.name.eq_ignore_ascii_case("track")).or(mdl.animations.first());
        if let Some(a) = anim {
            track.length = a.length.max(0.0);
            if let Some(node) = find_anim_node(&a.root_node, "modelhook") {
                for c in &node.controllers {
                    if c.controller_type == k2_mdl::MdlControllerType::POSITION {
                        track.pos = c.keys.iter().filter(|k| k.values.len() >= 3).map(|k| (k.time, rest_p + Vec3::new(k.values[0], k.values[1], k.values[2]))).collect();
                    } else if c.controller_type == k2_mdl::MdlControllerType::ORIENTATION {
                        track.rot = c
                            .keys
                            .iter()
                            .filter_map(|k| {
                                let q = match k.values.len() {
                                    1 => decompress_quat(k.values[0].to_bits()),
                                    n if n >= 4 => Quat::from_xyzw(k.values[0], k.values[1], k.values[2], k.values[3]),
                                    _ => return None,
                                };
                                Some((k.time, if q.length_squared() > 1e-8 { q.normalize() } else { Quat::IDENTITY }))
                            })
                            .collect();
                    }
                }
            }
        }
        if track.pos.is_empty() {
            track.pos = vec![(0.0, rest_p)];
        }
        if track.rot.is_empty() {
            track.rot = vec![(0.0, rest_r)];
        }
        let mut acc = 0.0;
        track.arc = track
            .pos
            .iter()
            .enumerate()
            .map(|(i, (_, p))| {
                if i > 0 {
                    acc += p.distance(track.pos[i - 1].1);
                }
                acc
            })
            .collect();
        track
    }

    pub fn distance(&self) -> f32 {
        self.arc.last().copied().unwrap_or(0.0)
    }

    /// World position and orientation at animation time `t`.
    pub fn sample(&self, t: f32) -> (Vec3, Quat) {
        let p = sample_keys(&self.pos, t, |a, b, f| a.lerp(b, f)).unwrap_or(Vec3::ZERO);
        let r = sample_keys(&self.rot, t, |a, b, f| a.slerp(b, f)).unwrap_or(Quat::IDENTITY);
        (self.origin + p, r)
    }

    /// Animation time at which the hook has travelled `d` along the path.
    pub fn time_at_distance(&self, d: f32) -> f32 {
        if self.arc.len() < 2 {
            return 0.0;
        }
        let d = d.clamp(0.0, self.distance());
        let i = self.arc.partition_point(|a| *a <= d).clamp(1, self.arc.len() - 1);
        let (a0, a1) = (self.arc[i - 1], self.arc[i]);
        let f = if a1 > a0 { (d - a0) / (a1 - a0) } else { 0.0 };
        let (t0, t1) = (self.pos[i - 1].0, self.pos[i].0);
        t0 + (t1 - t0) * f
    }

    /// Path distance at animation time `t` (inverse of time_at_distance).
    pub fn distance_at_time(&self, t: f32) -> f32 {
        if self.pos.len() < 2 {
            return 0.0;
        }
        let i = self.pos.partition_point(|k| k.0 <= t).clamp(1, self.pos.len() - 1);
        let (t0, t1) = (self.pos[i - 1].0, self.pos[i].0);
        let f = if t1 > t0 { ((t - t0) / (t1 - t0)).clamp(0.0, 1.0) } else { 0.0 };
        self.arc[i - 1] + (self.arc[i] - self.arc[i - 1]) * f
    }
}

fn sample_keys<T: Copy>(keys: &[(f32, T)], t: f32, mix: impl Fn(T, T, f32) -> T) -> Option<T> {
    let n = keys.len();
    if n == 0 {
        return None;
    }
    if n == 1 || t <= keys[0].0 {
        return Some(keys[0].1);
    }
    if t >= keys[n - 1].0 {
        return Some(keys[n - 1].1);
    }
    let i = keys.partition_point(|k| k.0 <= t) - 1;
    let (t0, t1) = (keys[i].0, keys[i + 1].0);
    let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
    Some(mix(keys[i].1, keys[i + 1].1, f))
}

/// Packed quaternion: x and y in 11 bits each, z in 10 bits, w implied.
fn decompress_quat(c: u32) -> Quat {
    let x = (c & 0x7FF) as f32 / 1023.0 - 1.0;
    let y = ((c >> 11) & 0x7FF) as f32 / 1023.0 - 1.0;
    let z = (c >> 22) as f32 / 511.0 - 1.0;
    let m2 = x * x + y * y + z * z;
    let w = if m2 < 1.0 { (1.0 - m2).sqrt() } else { 0.0 };
    Quat::from_xyzw(x, y, z, w)
}

fn find_node<'a>(n: &'a k2_mdl::MdlNode, name: &str) -> Option<&'a k2_mdl::MdlNode> {
    if n.name.eq_ignore_ascii_case(name) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_node(c, name))
}

fn find_anim_node<'a>(n: &'a k2_mdl::MdlAnimNode, name: &str) -> Option<&'a k2_mdl::MdlAnimNode> {
    if n.name.eq_ignore_ascii_case(name) {
        return Some(n);
    }
    n.children.iter().find_map(|c| find_anim_node(c, name))
}

fn load_mdl(world: &World, name: &str) -> Option<k2_mdl::Mdl> {
    let mdl = k2_data::read_bytes(&world.res, name, ResourceType::Mdl).ok()?;
    let mdx = k2_data::read_bytes(&world.res, name, ResourceType::Mdx).ok();
    k2_mdl::read_mdl_from_bytes(&mdl, mdx.as_deref()).ok()
}

/// Model-space bounding box of every mesh, offset by node translations.
fn model_bounds(mdl: &k2_mdl::Mdl) -> Option<(Vec3, Vec3)> {
    fn walk(n: &k2_mdl::MdlNode, at: Vec3, out: &mut Option<(Vec3, Vec3)>) {
        let at = at + Vec3::from(n.position);
        if let Some(m) = n.node_data.mesh() {
            let (lo, hi) = (at + Vec3::from(m.bounding_min), at + Vec3::from(m.bounding_max));
            if lo.is_finite() && hi.is_finite() && (hi - lo).length_squared() > 0.0 {
                *out = Some(match *out {
                    Some((a, b)) => (a.min(lo), b.max(hi)),
                    None => (lo, hi),
                });
            }
        }
        for c in &n.children {
            walk(c, at, out);
        }
    }
    let mut out = None;
    walk(&mdl.root_node, Vec3::ZERO, &mut out);
    out
}

/// Model-space collision triangles (node transforms applied): the AABB
/// (walkmesh) nodes when the model has any, else every rendered mesh.
fn model_triangles(mdl: &k2_mdl::Mdl) -> Vec<[Vec3; 3]> {
    let mut aabb = Vec::new();
    collect_triangles(&mdl.root_node, (Vec3::ZERO, Quat::IDENTITY), &mut aabb, true);
    if !aabb.is_empty() {
        return aabb;
    }
    let mut all = Vec::new();
    collect_triangles(&mdl.root_node, (Vec3::ZERO, Quat::IDENTITY), &mut all, false);
    all
}

fn collect_triangles(n: &k2_mdl::MdlNode, parent: (Vec3, Quat), out: &mut Vec<[Vec3; 3]>, aabb_only: bool) {
    let r = n.rotation;
    let local = Quat::from_xyzw(r[1], r[2], r[3], r[0]);
    let local = if local.length_squared() > 1e-8 { local.normalize() } else { Quat::IDENTITY };
    let (pp, pr) = parent;
    let at = (pp + pr * Vec3::from(n.position), pr * local);
    let take = match &n.node_data {
        k2_mdl::MdlNodeData::Aabb(_) => true,
        d => !aabb_only && d.mesh().is_some_and(|m| m.render),
    };
    if take && let Some(m) = n.node_data.mesh() {
        for f in &m.faces {
            let v = |i: u16| m.positions.get(i as usize).map(|p| at.0 + at.1 * Vec3::from(*p));
            if let (Some(a), Some(b), Some(c)) = (v(f.vertex_indices[0]), v(f.vertex_indices[1]), v(f.vertex_indices[2])) {
                out.push([a, b, c]);
            }
        }
    }
    for c in &n.children {
        collect_triangles(c, at, out, aabb_only);
    }
}

/// Model-space triangles of the meshes the renderer draws (render flag set,
/// not the AABB walkmesh), node transforms applied.
fn rendered_triangles(mdl: &k2_mdl::Mdl) -> Vec<[Vec3; 3]> {
    fn walk(n: &k2_mdl::MdlNode, parent: (Vec3, Quat), out: &mut Vec<[Vec3; 3]>) {
        let r = n.rotation;
        let local = Quat::from_xyzw(r[1], r[2], r[3], r[0]);
        let local = if local.length_squared() > 1e-8 { local.normalize() } else { Quat::IDENTITY };
        let (pp, pr) = parent;
        let at = (pp + pr * Vec3::from(n.position), pr * local);
        if !matches!(n.node_data, k2_mdl::MdlNodeData::Aabb(_))
            && !n.name.eq_ignore_ascii_case("trans")
            && let Some(m) = n.node_data.mesh().filter(|m| m.render)
        {
            for f in &m.faces {
                let v = |i: u16| m.positions.get(i as usize).map(|p| at.0 + at.1 * Vec3::from(*p));
                if let (Some(a), Some(b), Some(c)) = (v(f.vertex_indices[0]), v(f.vertex_indices[1]), v(f.vertex_indices[2])) {
                    out.push([a, b, c]);
                }
            }
        }
        for c in &n.children {
            walk(c, at, out);
        }
    }
    let mut out = Vec::new();
    walk(&mdl.root_node, (Vec3::ZERO, Quat::IDENTITY), &mut out);
    out
}

fn tri_bounds(t: &[[Vec3; 3]]) -> Option<(Vec3, Vec3)> {
    t.iter().flatten().fold(None, |acc: Option<(Vec3, Vec3)>, v| Some(acc.map_or((*v, *v), |(lo, hi)| (lo.min(*v), hi.max(*v)))))
}

#[allow(dead_code)]
fn model_triangles_all(mdl: &k2_mdl::Mdl) -> Vec<[Vec3; 3]> {
    fn walk(n: &k2_mdl::MdlNode, parent: (Vec3, Quat), out: &mut Vec<[Vec3; 3]>) {
        let r = n.rotation;
        let local = Quat::from_xyzw(r[1], r[2], r[3], r[0]);
        let local = if local.length_squared() > 1e-8 { local.normalize() } else { Quat::IDENTITY };
        let (pp, pr) = parent;
        let at = (pp + pr * Vec3::from(n.position), pr * local);
        if let Some(m) = n.node_data.mesh() {
            for f in &m.faces {
                let v = |i: u16| m.positions.get(i as usize).map(|p| at.0 + at.1 * Vec3::from(*p));
                if let (Some(a), Some(b), Some(c)) = (v(f.vertex_indices[0]), v(f.vertex_indices[1]), v(f.vertex_indices[2])) {
                    out.push([a, b, c]);
                }
            }
        }
        for c in &n.children {
            walk(c, at, out);
        }
    }
    let mut out = Vec::new();
    walk(&mdl.root_node, (Vec3::ZERO, Quat::IDENTITY), &mut out);
    out
}

/// Closest point on triangle abc to p (Ericson, Real-Time Collision Detection).
fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}

#[derive(Debug, Clone, Default)]
pub struct MgScripts {
    pub on_create: String,
    pub on_heartbeat: String,
    pub on_damage: String,
    pub on_death: String,
    pub on_fire: String,
    pub on_hit_bullet: String,
    pub on_hit_follower: String,
    pub on_hit_obstacle: String,
    pub on_track_loop: String,
    pub on_anim_event: String,
    pub on_accelerate: String,
    pub on_brake: String,
    pub on_hit_world: String,
}

impl MgScripts {
    fn read(s: Option<&GffStruct>) -> Self {
        let Some(s) = s else { return Self::default() };
        let r = |l: &str| s.resref(l).unwrap_or_default().to_ascii_lowercase();
        MgScripts {
            on_create: r("OnCreate"),
            on_heartbeat: r("OnHeartbeat"),
            on_damage: r("OnDamage"),
            on_death: r("OnDeath"),
            on_fire: r("OnFire"),
            on_hit_bullet: r("OnHitBullet"),
            on_hit_follower: r("OnHitFollower"),
            on_hit_obstacle: r("OnHitObstacle"),
            on_track_loop: r("OnTrackLoop"),
            on_anim_event: r("OnAnimEvent"),
            on_accelerate: r("OnAccelerate"),
            on_brake: r("OnBrake"),
            on_hit_world: r("OnHitWorld"),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct GunBank {
    pub id: u32,
    pub gun_model: String,
    pub fire_sound: String,
    pub damage: i32,
    pub lifespan: f32,
    pub bullet_model: String,
    pub rate_of_fire: f32,
    pub collision_sound: String,
    pub speed: f32,
    pub target_type: u32,
    pub inaccuracy: f32,
    pub sensing_radius: f32,
    pub horiz_spread: f32,
    pub vert_spread: f32,
    pub cooldown: f32,
}

impl GunBank {
    fn read(s: &GffStruct) -> Self {
        let b = s.child("Bullet");
        let bf = |l: &str, d: f32| b.map_or(d, |b| b.float_or(l, d));
        let br = |l: &str| b.and_then(|b| b.resref(l)).unwrap_or_default().to_ascii_lowercase();
        GunBank {
            id: s.uint("BankID").unwrap_or(0),
            gun_model: s.resref("Gun_Model").unwrap_or_default().to_ascii_lowercase(),
            fire_sound: s.resref("Fire_Sound").unwrap_or_default().to_ascii_lowercase(),
            damage: b.and_then(|b| b.int("Damage")).unwrap_or(0),
            lifespan: bf("Lifespan", 2.0),
            bullet_model: br("Bullet_Model"),
            rate_of_fire: bf("Rate_Of_Fire", 1.0),
            collision_sound: br("Collision_Sound"),
            speed: bf("Speed", 100.0),
            target_type: b.and_then(|b| b.uint("Target_Type")).unwrap_or(0),
            inaccuracy: s.float_or("Inaccuracy", 0.0),
            sensing_radius: s.float_or("Sensing_Radius", 0.0),
            horiz_spread: s.float_or("Horiz_Spread", 0.0),
            vert_spread: s.float_or("Vert_Spread", 0.0),
            cooldown: 0.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MgRole {
    #[default]
    Player,
    Enemy,
    Obstacle,
}

#[derive(Debug, Clone, Default)]
pub struct MgModel {
    pub name: String,
    /// Turns with the player's aim (turret) or steering (swoop).
    pub rotating: bool,
}

/// A minigame object (follower or obstacle).
#[derive(Debug, Clone, Default)]
pub struct MgObject {
    pub id: ObjectId,
    pub role: MgRole,
    /// Track resref for followers, model name for obstacles.
    pub name: String,
    pub track: Track,
    pub track_time: f32,
    /// Remaining loops (-1 = forever).
    pub loops: i32,
    pub models: Vec<MgModel>,
    pub sphere_radius: f32,
    pub hp: i32,
    pub max_hp: i32,
    pub invince_period: f32,
    pub invulnerable_for: f32,
    pub bump_damage: i32,
    pub death_sound: String,
    pub engine_sound: String,
    pub scripts: MgScripts,
    pub gun_banks: Vec<GunBank>,
    pub trigger: bool,
    pub alive: bool,
    /// Destroyed (SWMG_DestroyMiniGameObject or dead and gone).
    pub removed: bool,
    pub position: Vec3,
    pub rotation: Quat,
    /// Obstacles: world-space box.
    pub bounds: Option<(Vec3, Vec3)>,
    /// Obstacles: world-space collision triangles.
    pub triangles: Vec<[Vec3; 3]>,
}

impl MgObject {
    fn read_follower(world: &mut World, s: &GffStruct, role: MgRole) -> MgObject {
        let track_name = s.resref("Track").unwrap_or_default().to_ascii_lowercase();
        let track = Track::load(world, &track_name);
        let sounds = s.child("Sounds");
        let snd = |l: &str| sounds.and_then(|x| x.resref(l)).unwrap_or_default().to_ascii_lowercase();
        let hp = s.int("Hit_Points").unwrap_or(1);
        let mut o = MgObject {
            id: world.alloc_id(),
            role,
            name: track_name,
            track,
            track_time: 0.0,
            loops: s.int("Num_Loops").unwrap_or(-1),
            models: s
                .list("Models")
                .iter()
                .map(|m| MgModel { name: m.resref("Model").unwrap_or_default().to_ascii_lowercase(), rotating: m.flag("RotatingModel") })
                .filter(|m| !m.name.is_empty())
                .collect(),
            sphere_radius: s.float_or("Sphere_Radius", 1.0),
            hp,
            max_hp: s.int("Max_HPs").unwrap_or(hp).max(1),
            invince_period: s.float_or("Invince_Period", 0.0),
            invulnerable_for: 0.0,
            bump_damage: s.int("Bump_Damage").unwrap_or(0),
            death_sound: snd("Death"),
            engine_sound: snd("Engine"),
            scripts: MgScripts::read(s.child("Scripts")),
            gun_banks: s.list("Gun_Banks").iter().map(GunBank::read).collect(),
            trigger: s.flag("Trigger"),
            alive: true,
            removed: false,
            position: Vec3::ZERO,
            rotation: Quat::IDENTITY,
            bounds: None,
            triangles: Vec::new(),
        };
        let (p, r) = o.track.sample(0.0);
        o.position = p;
        o.rotation = r;
        o
    }

    pub fn is_live(&self) -> bool {
        self.alive && !self.removed
    }
}

/// Input the game layer feeds each frame.
#[derive(Debug, Clone, Copy, Default)]
pub struct MgInput {
    /// Swoop steering, -1 (left) .. 1 (right).
    pub lateral: f32,
    /// Turret aim change this frame in degrees: x = mouse X, y = mouse Y.
    pub aim: Vec2,
    /// Fire held.
    pub fire: bool,
    /// Accelerate (gear shift) pressed this frame.
    pub accelerate: bool,
    /// Brake / jump pressed this frame.
    pub brake: bool,
}

/// Things the game layer shows or plays.
#[derive(Debug, Clone, PartialEq)]
pub enum MgEvent {
    /// SWMG_PlayAnimation on every model of `object` that has it.
    Animation { object: ObjectId, anim: String, looping: bool, queue: bool, overlay: bool },
    RemoveAnimation { object: ObjectId, anim: String },
    Sound { sound: String, position: Vec3 },
    Removed { object: ObjectId },
    SpeedBlur { enabled: bool, ratio: f32 },
}

#[derive(Debug, Clone)]
pub struct Bullet {
    pub id: u32,
    pub shooter: ObjectId,
    pub model: String,
    pub position: Vec3,
    pub velocity: Vec3,
    pub life: f32,
    pub damage: i32,
    pub target_type: u32,
    pub collision_sound: String,
}

/// The current module's minigame.
#[derive(Debug, Clone, Default)]
pub struct MiniGame {
    pub kind: u32,
    pub near_clip: f32,
    pub far_clip: f32,
    pub view_angle: f32,
    pub movement_per_sec: f32,
    pub lateral_accel: f32,
    pub use_inertia: bool,
    pub do_bumping: bool,
    pub dof: u32,
    pub music: String,
    pub mouse_axis_x: u32,
    pub mouse_axis_y: u32,
    pub flip_x: bool,
    pub flip_y: bool,
    pub camera_model: String,
    pub camera_rotate: bool,
    pub player: MgObject,
    pub enemies: Vec<MgObject>,
    pub obstacles: Vec<MgObject>,
    pub bullets: Vec<Bullet>,
    // Player motion.
    pub speed: f32,
    pub min_speed: f32,
    pub max_speed: f32,
    pub accel_per_sec: f32,
    pub lateral_aps: f32,
    /// Swoop: x lateral, z height. Turret: aim angles in degrees.
    pub offset: Vec3,
    pub lateral_vel: f32,
    pub vertical_vel: f32,
    pub tunnel_pos: Vec3,
    pub tunnel_neg: Vec3,
    pub tunnel_infinite: Vec3,
    pub start_offset: Vec3,
    pub target_offset: Vec3,
    pub origin: Option<Vec3>,
    pub invincibility: f32,
    pub distance: f32,
    pub input: MgInput,
    // SWMG_GetLast*.
    pub last_bullet_hit_damage: i32,
    pub last_bullet_hit_target: i32,
    pub last_bullet_hit_shooter: ObjectId,
    pub last_bullet_fired_damage: i32,
    pub last_bullet_fired_target: i32,
    pub last_follower_hit: ObjectId,
    pub last_obstacle_hit: ObjectId,
    pub last_hp_change: i32,
    pub last_event: String,
    pub last_event_model: String,
    /// Followers/obstacles the player currently touches (hits fire on contact).
    touching: HashSet<ObjectId>,
    pub events: Vec<MgEvent>,
    heartbeat_in: f32,
    pub elapsed: f32,
    next_bullet: u32,
    rng: u64,
    /// Seconds until the next automatic gear shift may fire.
    shift_in: f32,
    /// Gear and max speed at the last shift (to notice a shift that did nothing).
    shifted_from: Option<(i32, f32)>,
    /// Highest gear (a shift from it changes nothing), once discovered.
    top_gear: Option<i32>,
}

impl MiniGame {
    pub fn is_turret(&self) -> bool {
        self.kind == KIND_TURRET
    }

    pub fn object(&self, id: ObjectId) -> Option<&MgObject> {
        std::iter::once(&self.player).chain(&self.enemies).chain(&self.obstacles).find(|o| o.id == id)
    }

    fn object_mut(&mut self, id: ObjectId) -> Option<&mut MgObject> {
        std::iter::once(&mut self.player).chain(&mut self.enemies).chain(&mut self.obstacles).find(|o| o.id == id)
    }

    /// Turret aim direction (game space) from the offset angles.
    pub fn aim_direction(&self) -> Vec3 {
        let (base_pos, base_rot) = (self.player.position, self.player.rotation);
        let _ = base_pos;
        let yaw = self.offset.z.to_radians();
        let pitch = self.offset.x.to_radians();
        // Track forward is +Y; yaw about +Z, pitch about the right axis.
        let local = Quat::from_rotation_z(yaw) * Quat::from_rotation_x(pitch);
        (base_rot * local * Vec3::Y).normalize_or_zero()
    }

    /// The player's rotation including aim (turret) or bank (swoop).
    pub fn player_aim_rotation(&self) -> Quat {
        if self.is_turret() {
            self.player.rotation * Quat::from_rotation_z(self.offset.z.to_radians()) * Quat::from_rotation_x(self.offset.x.to_radians())
        } else {
            self.player.rotation
        }
    }

    /// Nearest obstacle face within `r` of `p`: (obstacle, distance, top of
    /// that obstacle's box). For steering and jump decisions.
    pub fn obstacle_near(&self, p: Vec3, r: f32) -> Option<(ObjectId, f32, f32)> {
        let mut best: Option<(ObjectId, f32, f32)> = None;
        for o in self.obstacles.iter().filter(|o| !o.trigger) {
            let Some((lo, hi)) = o.bounds else { continue };
            if p.clamp(lo, hi).distance(p) > r {
                continue;
            }
            for t in &o.triangles {
                let d = closest_on_triangle(p, t[0], t[1], t[2]).distance(p);
                if d <= r && best.is_none_or(|b| d < b.1) {
                    best = Some((o.id, d, hi.z));
                }
            }
        }
        best
    }

    /// Player position if it were `ahead` units further along its track at
    /// lateral offset `x`.
    pub fn player_position_ahead(&self, ahead: f32, x: f32) -> Vec3 {
        let tr = &self.player.track;
        let (p, r) = tr.sample(tr.time_at_distance(self.distance + ahead));
        p + r * (self.start_offset + Vec3::new(x, 0.0, self.offset.z))
    }

    fn rand(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        (self.rng % 10_000) as f32 / 10_000.0
    }

    fn anim(&mut self, object: ObjectId, anim: &str, looping: bool) {
        self.events.push(MgEvent::Animation { object, anim: anim.to_ascii_lowercase(), looping, queue: false, overlay: false });
    }
}

impl World {
    /// Builds the minigame of the module just entered (none if its ARE has
    /// no MiniGame struct) and runs the OnCreate scripts.
    pub(crate) fn start_minigame(&mut self) {
        self.minigame = None;
        // Debug switch: run minigame modules as plain areas.
        if std::env::var_os("K2_NO_MINIGAME").is_some() {
            return;
        }
        let Some(area) = self.module.as_ref().map(|m| m.area.resref.clone()) else { return };
        let Ok(are) = k2_data::read_gff(&self.res, &area, ResourceType::Are) else { return };
        let Some(mg) = are.root.child("MiniGame") else { return };
        let kind = mg.uint("Type").unwrap_or(0);
        if kind == 0 {
            return;
        }
        let mouse = mg.child("Mouse");
        let Some(ps) = mg.child("Player") else { return };
        let player = MgObject::read_follower(self, ps, MgRole::Player);
        let enemies: Vec<MgObject> = mg.list("Enemies").to_vec().iter().map(|e| MgObject::read_follower(self, e, MgRole::Enemy)).collect();
        let obstacle_defs = mg.list("Obstacles").to_vec();
        let mut obstacles = Vec::new();
        for o in &obstacle_defs {
            let name = o.resref("Name").unwrap_or_default().to_ascii_lowercase();
            let origin = self
                .module
                .as_ref()
                .and_then(|m| m.layout.obstacles.iter().find(|t| t.model.eq_ignore_ascii_case(&name)))
                .map(|t| Vec3::from(t.position));
            let Some(origin) = origin else { continue };
            let mdl = load_mdl(self, &name);
            // Only wall-like faces block: the swoop rides on floor faces.
            let triangles: Vec<[Vec3; 3]> = mdl
                .as_ref()
                .map(model_triangles)
                .unwrap_or_default()
                .into_iter()
                .filter(|t| (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero().z.abs() < 0.7)
                .map(|t| t.map(|v| v + origin))
                .collect();
            let bounds = triangles.iter().flatten().fold(None, |acc: Option<(Vec3, Vec3)>, v| Some(acc.map_or((*v, *v), |(lo, hi)| (lo.min(*v), hi.max(*v)))));
            let bounds = bounds.or_else(|| mdl.as_ref().and_then(model_bounds).map(|(a, b)| (origin + a, origin + b)));
            let scripts = MgScripts::read(o.child("Scripts"));
            obstacles.push(MgObject {
                id: self.alloc_id(),
                role: MgRole::Obstacle,
                name: name.clone(),
                models: vec![MgModel { name, rotating: false }],
                // An obstacle with an OnHitFollower script is an invisible
                // trigger volume, not a wall: the event props (mge: play
                // the "hit" animation of the freighter or gate) and the
                // force fields (mgf: push the swoop sideways or up) have
                // one, the crates and pillars (mgc) never do.
                trigger: !scripts.on_hit_follower.is_empty(),
                scripts,
                alive: true,
                position: origin,
                rotation: Quat::IDENTITY,
                bounds,
                triangles,
                ..Default::default()
            });
        }
        let v3 = |s: &GffStruct, x: &str, y: &str, z: &str| Vec3::new(s.float_or(x, 0.0), s.float_or(y, 0.0), s.float_or(z, 0.0));
        let tunnel_infinite = match ps.value("TunnelInfinite") {
            Some(GffValue::Vector3(v)) => Vec3::from(*v),
            _ => Vec3::ZERO,
        };
        let game = MiniGame {
            kind,
            near_clip: mg.float_or("Near_Clip", 0.1),
            far_clip: mg.float_or("Far_Clip", 10000.0),
            view_angle: mg.float_or("CameraViewAngle", 65.0),
            movement_per_sec: mg.float_or("MovementPerSec", 100.0),
            lateral_accel: mg.float_or("LateralAccel", 300.0),
            use_inertia: mg.flag("UseInertia"),
            do_bumping: mg.flag("DoBumping"),
            dof: mg.uint("DOF").unwrap_or(0),
            music: mg.resref("Music").unwrap_or_default().to_ascii_lowercase(),
            mouse_axis_x: mouse.and_then(|m| m.uint("AxisX")).unwrap_or(0),
            mouse_axis_y: mouse.and_then(|m| m.uint("AxisY")).unwrap_or(0),
            flip_x: mouse.is_some_and(|m| m.flag("FlipAxisX")),
            flip_y: mouse.is_some_and(|m| m.flag("FlipAxisY")),
            camera_model: ps.resref("Camera").unwrap_or_default().to_ascii_lowercase(),
            camera_rotate: ps.flag("CameraRotate"),
            min_speed: ps.float_or("Minimum_Speed", 0.0),
            max_speed: ps.float_or("Maximum_Speed", 0.0),
            accel_per_sec: 0.0,
            lateral_aps: mg.float_or("LateralAccel", 300.0),
            tunnel_pos: v3(ps, "TunnelXPos", "TunnelYPos", "TunnelZPos"),
            tunnel_neg: v3(ps, "TunnelXNeg", "TunnelYNeg", "TunnelZNeg"),
            tunnel_infinite,
            start_offset: v3(ps, "Start_Offset_X", "Start_Offset_Y", "Start_Offset_Z"),
            target_offset: v3(ps, "Target_Offset_X", "Target_Offset_Y", "Target_Offset_Z"),
            player,
            enemies,
            obstacles,
            last_bullet_hit_shooter: OBJECT_INVALID,
            last_follower_hit: OBJECT_INVALID,
            last_obstacle_hit: OBJECT_INVALID,
            rng: 0x2545_F491_4F6C_DD1D ^ (self.time.to_bits()),
            ..Default::default()
        };
        if self.trace {
            println!(
                "minigame {} type {kind}: {} enemies, {} obstacles, player track {} ({:.0} units)",
                area,
                game.enemies.len(),
                game.obstacles.len(),
                game.player.track.name,
                game.player.track.distance()
            );
        }
        let creates: Vec<(ObjectId, String)> =
            std::iter::once(&game.player).chain(&game.enemies).map(|o| (o.id, o.scripts.on_create.clone())).filter(|(_, s)| !s.is_empty()).collect();
        self.minigame = Some(game);
        for (id, script) in creates {
            self.mg_script(&script, id);
        }
    }

    fn mg_script(&mut self, script: &str, caller: ObjectId) {
        if script.is_empty() {
            return;
        }
        self.run_script(script, caller, ScriptCtx::default());
        self.pump();
    }

    /// Feeds this frame's input (the game layer).
    pub fn minigame_input(&mut self, input: MgInput) {
        if let Some(mg) = self.minigame.as_mut() {
            let keep_edges = (mg.input.accelerate || input.accelerate, mg.input.brake || input.brake);
            mg.input = input;
            mg.input.accelerate = keep_edges.0;
            mg.input.brake = keep_edges.1;
        }
    }

    pub fn drain_minigame_events(&mut self) -> Vec<MgEvent> {
        self.minigame.as_mut().map(|m| std::mem::take(&mut m.events)).unwrap_or_default()
    }

    pub(crate) fn tick_minigame(&mut self, dt: f32) {
        let Some(mut mg) = self.minigame.take() else { return };
        if dt <= 0.0 {
            self.minigame = Some(mg);
            return;
        }
        let mut scripts: Vec<(String, ObjectId)> = Vec::new();
        mg.elapsed += dt;
        let mut input = std::mem::take(&mut mg.input);
        // Swoop gears shift with the accelerate key (the OnAccelerate script
        // checks the speed: too early is a penalty). The harness and the
        // `--mg-auto` autopilot (`auto_gears`) press it for the player as
        // soon as the current gear's top speed is reached.
        input.accelerate = if mg.is_turret() || !self.auto_gears { input.accelerate } else { self.auto_shift(&mut mg, dt, input.accelerate) };
        // Edge inputs run their scripts.
        if input.accelerate {
            scripts.push((mg.player.scripts.on_accelerate.clone(), mg.player.id));
        }
        if input.brake {
            scripts.push((mg.player.scripts.on_brake.clone(), mg.player.id));
        }
        // Held inputs carry to the next frame unless replaced.
        mg.input = MgInput { lateral: input.lateral, aim: Vec2::ZERO, fire: input.fire, ..Default::default() };

        // Player motion.
        if mg.is_turret() {
            let mut ax = [0.0f32; 4];
            let dx = if mg.flip_x { -input.aim.x } else { input.aim.x };
            let dy = if mg.flip_y { -input.aim.y } else { input.aim.y };
            if (1..=3).contains(&mg.mouse_axis_x) {
                ax[mg.mouse_axis_x as usize] += dx;
            }
            if (1..=3).contains(&mg.mouse_axis_y) {
                ax[mg.mouse_axis_y as usize] += dy;
            }
            mg.offset += Vec3::new(ax[1], ax[2], ax[3]);
            clamp_offset(&mut mg);
            let (p, r) = mg.player.track.sample(mg.player.track_time);
            mg.player.track_time += dt;
            mg.player.position = mg.origin.unwrap_or(p) + r * mg.start_offset;
            mg.player.rotation = r;
        } else {
            mg.speed += mg.accel_per_sec * dt;
            if mg.max_speed > 0.0 {
                mg.speed = mg.speed.min(mg.max_speed);
            }
            mg.speed = mg.speed.max(mg.min_speed).max(0.0);
            mg.distance += mg.speed * dt;
            let total = mg.player.track.distance();
            if total > 0.0 && mg.distance >= total {
                if mg.player.loops != 0 {
                    mg.distance -= total;
                    if mg.player.loops > 0 {
                        mg.player.loops -= 1;
                    }
                    scripts.push((mg.player.scripts.on_track_loop.clone(), mg.player.id));
                } else {
                    mg.distance = total;
                }
            }
            mg.player.track_time = mg.player.track.time_at_distance(mg.distance);
            // Steering: sideways acceleration toward a peak speed, braking
            // when released.
            let steer = mg.lateral_aps > 0.0 && input.lateral != 0.0;
            let width = mg.tunnel_pos.x.abs() + mg.tunnel_neg.x.abs();
            let peak = (width / CROSSING_SECS).clamp(LATERAL_MAX_SPEED.0, LATERAL_MAX_SPEED.1);
            if !mg.use_inertia {
                mg.lateral_vel = if mg.lateral_aps > 0.0 { input.lateral * peak } else { 0.0 };
            } else if steer {
                let accel = mg.lateral_aps.clamp(LATERAL_ACCEL_RANGE.0, LATERAL_ACCEL_RANGE.1);
                let reversing = mg.lateral_vel * input.lateral < 0.0;
                let accel = if reversing { accel * LATERAL_REVERSE_BOOST } else { accel };
                mg.lateral_vel = (mg.lateral_vel + input.lateral * accel * dt).clamp(-peak, peak);
            } else {
                mg.lateral_vel *= (-LATERAL_DAMPING * dt).exp();
                if mg.lateral_vel.abs() < 0.05 {
                    mg.lateral_vel = 0.0;
                }
            }
            mg.offset.x += mg.lateral_vel * dt;
            // Jumps fall back to the track.
            mg.vertical_vel -= GRAVITY * dt;
            mg.offset.z += mg.vertical_vel * dt;
            if mg.offset.z <= 0.0 {
                mg.offset.z = 0.0;
                mg.vertical_vel = 0.0;
            }
            let before = mg.offset.x;
            clamp_offset(&mut mg);
            if mg.offset.x != before && (mg.offset.x - before) * mg.lateral_vel < 0.0 {
                // Against the tunnel wall: no sideways speed left to wind up.
                mg.lateral_vel = 0.0;
            }
            let (p, r) = mg.player.track.sample(mg.player.track_time);
            mg.player.position = mg.origin.unwrap_or(p) + r * (mg.start_offset + mg.offset);
            mg.player.rotation = r;
        }
        mg.player.invulnerable_for = (mg.player.invulnerable_for - dt).max(0.0);
        mg.invincibility = (mg.invincibility - dt).max(0.0);

        // Enemies follow their tracks.
        for e in &mut mg.enemies {
            if !e.is_live() {
                continue;
            }
            e.invulnerable_for = (e.invulnerable_for - dt).max(0.0);
            if e.track.length > 0.0 {
                e.track_time += dt;
                if e.track_time > e.track.length {
                    if e.loops != 0 {
                        e.track_time -= e.track.length;
                        if e.loops > 0 {
                            e.loops -= 1;
                        }
                        scripts.push((e.scripts.on_track_loop.clone(), e.id));
                    } else {
                        e.track_time = e.track.length;
                    }
                }
            }
            let (p, r) = e.track.sample(e.track_time);
            e.position = p;
            e.rotation = r;
        }

        // Guns.
        let aim = mg.aim_direction();
        let player_pos = mg.player.position;
        let player_id = mg.player.id;
        if input.fire && mg.player.alive {
            let mut fired = false;
            for b in 0..mg.player.gun_banks.len() {
                let g = &mut mg.player.gun_banks[b];
                if g.cooldown > 0.0 {
                    continue;
                }
                g.cooldown = g.rate_of_fire.max(0.05);
                let (model, speed, life, dmg, tt, cs, fs) =
                    (g.bullet_model.clone(), g.speed, g.lifespan, g.damage, g.target_type, g.collision_sound.clone(), g.fire_sound.clone());
                mg.next_bullet += 1;
                let id = mg.next_bullet;
                mg.bullets.push(Bullet {
                    id,
                    shooter: player_id,
                    model,
                    position: player_pos + aim * (mg.player.sphere_radius * 0.5),
                    velocity: aim * speed,
                    life,
                    damage: dmg,
                    target_type: tt,
                    collision_sound: cs,
                });
                mg.last_bullet_fired_damage = dmg;
                mg.last_bullet_fired_target = tt as i32;
                if !fs.is_empty() {
                    mg.events.push(MgEvent::Sound { sound: fs, position: player_pos });
                }
                fired = true;
            }
            if fired {
                mg.anim(player_id, "fire", false);
                scripts.push((mg.player.scripts.on_fire.clone(), player_id));
            }
        }
        for g in &mut mg.player.gun_banks {
            g.cooldown = (g.cooldown - dt).max(0.0);
        }
        // Enemy guns fire at the player inside their sensing radius.
        let mut enemy_shots = Vec::new();
        for (i, e) in mg.enemies.iter_mut().enumerate() {
            if !e.is_live() {
                continue;
            }
            for (b, g) in e.gun_banks.iter_mut().enumerate() {
                g.cooldown = (g.cooldown - dt).max(0.0);
                if g.target_type != TARGET_PLAYER || g.cooldown > 0.0 || g.sensing_radius <= 0.0 {
                    continue;
                }
                if e.position.distance(player_pos) <= g.sensing_radius {
                    g.cooldown = g.rate_of_fire.max(0.05);
                    enemy_shots.push((i, b));
                }
            }
        }
        for (i, b) in enemy_shots {
            let (jx, jy) = (mg.rand() - 0.5, mg.rand() - 0.5);
            let e = &mg.enemies[i];
            let g = &e.gun_banks[b];
            let dir = (player_pos - e.position).normalize_or_zero();
            let side = dir.cross(Vec3::Z).normalize_or_zero();
            let up = side.cross(dir).normalize_or_zero();
            let spread = g.inaccuracy.max(0.0) * 10.0;
            let dir = (dir + side * jx * spread + up * jy * spread).normalize_or_zero();
            let bullet = Bullet {
                id: 0,
                shooter: e.id,
                model: g.bullet_model.clone(),
                position: e.position,
                velocity: dir * g.speed,
                life: g.lifespan,
                damage: g.damage,
                target_type: g.target_type,
                collision_sound: g.collision_sound.clone(),
            };
            let fs = g.fire_sound.clone();
            let pos = e.position;
            let eid = e.id;
            mg.next_bullet += 1;
            mg.bullets.push(Bullet { id: mg.next_bullet, ..bullet });
            if !fs.is_empty() {
                mg.events.push(MgEvent::Sound { sound: fs, position: pos });
            }
            mg.anim(eid, "fire", false);
        }

        // Bullets.
        let mut hits: Vec<(ObjectId, usize)> = Vec::new(); // target, bullet index
        for (bi, b) in mg.bullets.iter_mut().enumerate() {
            let from = b.position;
            b.position += b.velocity * dt;
            b.life -= dt;
            let targets: Vec<(ObjectId, Vec3, f32)> = match b.target_type {
                TARGET_PLAYER => vec![(mg.player.id, mg.player.position, mg.player.sphere_radius)],
                _ => mg.enemies.iter().filter(|e| e.is_live() && !e.trigger).map(|e| (e.id, e.position, e.sphere_radius)).collect(),
            };
            for (id, at, r) in targets {
                if segment_sphere(from, b.position, at, r) {
                    hits.push((id, bi));
                    b.life = -1.0;
                    break;
                }
            }
        }
        for (target, bi) in hits {
            let b = mg.bullets[bi].clone();
            mg.last_bullet_hit_damage = b.damage;
            mg.last_bullet_hit_target = b.target_type as i32;
            mg.last_bullet_hit_shooter = b.shooter;
            if !b.collision_sound.is_empty() {
                mg.events.push(MgEvent::Sound { sound: b.collision_sound.clone(), position: b.position });
            }
            let invulnerable = target == mg.player.id && mg.invincibility > 0.0;
            let Some(t) = mg.object_mut(target) else { continue };
            if invulnerable || t.invulnerable_for > 0.0 {
                continue;
            }
            t.hp -= b.damage;
            t.invulnerable_for = t.invince_period;
            let (dmg, death, dead) = (t.scripts.on_damage.clone(), t.scripts.on_death.clone(), t.hp <= 0 && t.alive);
            if !t.scripts.on_hit_bullet.is_empty() {
                scripts.push((t.scripts.on_hit_bullet.clone(), target));
            }
            mg.last_hp_change = -b.damage;
            if dmg.is_empty() {
                mg.anim(target, "damage", false);
            } else {
                scripts.push((dmg, target));
            }
            if dead && target != mg.player.id {
                if let Some(t) = mg.object_mut(target) {
                    t.alive = false;
                }
                scripts.push((if death.is_empty() { "<default-death>".into() } else { death }, target));
            }
        }
        mg.bullets.retain(|b| b.life > 0.0);

        // Swoop contacts.
        if !mg.is_turret() {
            let mut now = HashSet::new();
            let pr = mg.player.sphere_radius;
            for e in &mg.enemies {
                if e.is_live() && e.position.distance(player_pos) <= pr + e.sphere_radius {
                    now.insert(e.id);
                    if !mg.touching.contains(&e.id) {
                        // Pickups (accelerator pads) sound their "death" sound.
                        if e.trigger && !e.death_sound.is_empty() {
                            mg.events.push(MgEvent::Sound { sound: e.death_sound.clone(), position: e.position });
                        }
                        mg.last_follower_hit = e.id;
                        scripts.push((e.scripts.on_hit_follower.clone(), e.id));
                        scripts.push((mg.player.scripts.on_hit_follower.clone(), mg.player.id));
                    }
                }
            }
            let mut bump = None;
            for o in &mg.obstacles {
                let Some((lo, hi)) = o.bounds else { continue };
                if player_pos.clamp(lo, hi).distance(player_pos) > pr {
                    continue;
                }
                // Inside the box: test the actual triangles (big set pieces
                // have boxes that cover the whole track).
                let c = if o.triangles.is_empty() {
                    player_pos.clamp(lo, hi)
                } else {
                    o.triangles
                        .iter()
                        .map(|t| closest_on_triangle(player_pos, t[0], t[1], t[2]))
                        .min_by(|a, b| a.distance_squared(player_pos).total_cmp(&b.distance_squared(player_pos)))
                        .unwrap_or(player_pos + Vec3::splat(f32::MAX))
                };
                if c.distance(player_pos) <= pr {
                    now.insert(o.id);
                    if o.trigger {
                        // Trigger volume: its own script runs once on entry;
                        // no damage, no bump, no OnHitObstacle for the player.
                        if !mg.touching.contains(&o.id) {
                            scripts.push((o.scripts.on_hit_follower.clone(), o.id));
                        }
                        continue;
                    }
                    if !mg.touching.contains(&o.id) {
                        mg.last_obstacle_hit = o.id;
                        scripts.push((mg.player.scripts.on_hit_obstacle.clone(), mg.player.id));
                        scripts.push((o.scripts.on_hit_obstacle.clone(), o.id));
                        bump = Some((player_pos - c).normalize_or_zero());
                    }
                }
            }
            if let (Some(away), true) = (bump, mg.do_bumping) {
                // Push back toward the track and lose some speed.
                let right = mg.player.rotation * Vec3::X;
                let side = away.dot(right).signum();
                mg.lateral_vel = side * mg.lateral_aps.max(30.0) * 0.1;
                mg.speed *= 0.8;
                if mg.player.bump_damage > 0 && mg.invincibility <= 0.0 {
                    mg.player.hp -= mg.player.bump_damage;
                    mg.last_hp_change = -mg.player.bump_damage;
                }
            }
            mg.touching = now;
        }

        // Heartbeat.
        mg.heartbeat_in -= dt;
        if mg.heartbeat_in <= 0.0 {
            mg.heartbeat_in = HEARTBEAT;
            scripts.push((mg.player.scripts.on_heartbeat.clone(), mg.player.id));
        }

        // Test hook: K2_MG_CRASH=SECS runs the player's OnHitObstacle once,
        // SECS after the minigame starts (screenshots of the crash effects).
        if let Some(at) = std::env::var("K2_MG_CRASH").ok().and_then(|v| v.parse::<f32>().ok())
            && mg.elapsed - dt < at
            && at <= mg.elapsed
        {
            scripts.push((mg.player.scripts.on_hit_obstacle.clone(), mg.player.id));
        }

        self.minigame = Some(mg);
        for (script, caller) in scripts {
            if script == "<default-death>" {
                self.mg_default_death(caller);
            } else {
                self.mg_script(&script, caller);
            }
        }
    }

    /// Whether the swoop shifts up this tick (see `tick_minigame`).
    fn auto_shift(&self, mg: &mut MiniGame, dt: f32, pressed: bool) -> bool {
        mg.shift_in = (mg.shift_in - dt).max(0.0);
        let Some(gear) = self.globals.numbers.get(GEAR_GLOBAL).copied() else { return false };
        // A shift above the first that left the speed limit alone came from
        // the top gear (a shift after a crash resets: gear 0, same limit).
        if let Some((from, max)) = mg.shifted_from.take()
            && from >= 1
            && mg.max_speed == max
        {
            mg.top_gear = Some(from);
        }
        if gear < 0 || mg.shift_in > 0.0 || mg.top_gear.is_some_and(|t| gear >= t) {
            return false;
        }
        let at_speed = gear == 0 || (mg.max_speed > 0.0 && mg.speed >= mg.max_speed - 1.0);
        if !(pressed || at_speed) {
            return false;
        }
        mg.shift_in = SHIFT_COOLDOWN;
        mg.shifted_from = Some((gear, mg.max_speed));
        true
    }

    /// Node tree and animations of a model (debug probe).
    pub fn mdl_tree(&self, name: &str) -> Vec<String> {
        fn walk(n: &k2_mdl::MdlNode, depth: usize, out: &mut Vec<String>) {
            let kind = match &n.node_data {
                k2_mdl::MdlNodeData::Aabb(_) => "aabb",
                d if d.mesh().is_some() => "mesh",
                _ => "other",
            };
            let m = n.node_data.mesh();
            out.push(format!(
                "{}{} [{kind}] pos {:?} render {:?} faces {:?} bounds {:?}",
                "  ".repeat(depth),
                n.name,
                n.position,
                m.map(|m| m.render),
                m.map(|m| m.faces.len()),
                m.map(|m| (m.bounding_min, m.bounding_max))
            ));
            for c in &n.children {
                walk(c, depth + 1, out);
            }
        }
        fn anim(n: &k2_mdl::MdlAnimNode, out: &mut Vec<String>) {
            for c in &n.controllers {
                if c.controller_type == k2_mdl::MdlControllerType::POSITION {
                    let first = c.keys.first().map(|k| (k.time, k.values.clone()));
                    let last = c.keys.last().map(|k| (k.time, k.values.clone()));
                    out.push(format!("    node {} position keys {}: first {:?} last {:?}", n.name, c.keys.len(), first, last));
                }
            }
            for ch in &n.children {
                anim(ch, out);
            }
        }
        let mut out = Vec::new();
        let Some(mdl) = load_mdl(self, name) else { return vec![format!("{name}: not found")] };
        walk(&mdl.root_node, 0, &mut out);
        for a in &mdl.animations {
            out.push(format!("  animation {} length {:.2} root {}", a.name, a.length, a.anim_root));
            anim(&a.root_node, &mut out);
        }
        out
    }

    /// Lists every collidable minigame object whose collision volume has no
    /// drawn geometry behind it (a probe for `k2world minigame --audit`).
    pub fn audit_minigame(&self) -> Vec<String> {
        let mut out = Vec::new();
        let Some(mg) = self.minigame.as_ref() else { return out };
        let fmt = |b: Option<(Vec3, Vec3)>| b.map_or("none".to_string(), |(l, h)| format!("({:.0},{:.0},{:.0})..({:.0},{:.0},{:.0})", l.x, l.y, l.z, h.x, h.y, h.z));
        // Everything drawn in the area (room models at their LYT positions),
        // as world-space vertices: what a collision volume must sit on.
        let mut scenery: Vec<[Vec3; 3]> = Vec::new();
        if let Some(m) = self.module.as_ref() {
            for r in &m.layout.rooms {
                if let Some(mdl) = load_mdl(self, &r.model) {
                    let at = Vec3::from(r.position);
                    scenery.extend(rendered_triangles(&mdl).iter().map(|t| t.map(|v| v + at)));
                }
            }
        }
        // Distance from the faces of a collision box to the nearest drawn
        // scenery face (0 when the box is a drawn crate or pillar).
        let scenery_gap = |lo: Vec3, hi: Vec3| -> f32 {
            let pad = Vec3::splat(6.0);
            let near: Vec<&[Vec3; 3]> = scenery
                .iter()
                .filter(|t| {
                    let (a, b) = (t[0].min(t[1]).min(t[2]), t[0].max(t[1]).max(t[2]));
                    b.cmpge(lo - pad).all() && a.cmple(hi + pad).all()
                })
                .collect();
            let mut best = f32::MAX;
            let n = 4;
            for i in 0..=n {
                for j in 0..=n {
                    let (u, v) = (i as f32 / n as f32, j as f32 / n as f32);
                    let d = hi - lo;
                    let faces = [
                        lo + Vec3::new(d.x * u, d.y * v, 0.0),
                        lo + Vec3::new(d.x * u, d.y * v, d.z),
                        lo + Vec3::new(d.x * u, 0.0, d.z * v),
                        lo + Vec3::new(d.x * u, d.y, d.z * v),
                        lo + Vec3::new(0.0, d.y * u, d.z * v),
                        lo + Vec3::new(d.x, d.y * u, d.z * v),
                    ];
                    // The floor passes through the bottom of the box (z = the
                    // track): only points a little above it say anything.
                    for p in faces.into_iter().filter(|p| p.z >= lo.z + 2.0) {
                        for t in &near {
                            best = best.min(closest_on_triangle(p, t[0], t[1], t[2]).distance(p));
                        }
                    }
                }
            }
            best
        };
        for o in &mg.obstacles {
            let mdl = load_mdl(self, &o.name);
            let Some(mdl) = mdl else {
                out.push(format!("OBSTACLE {} at {:?}: model failed to load, {} collision triangles", o.name, o.position.to_array(), o.triangles.len()));
                continue;
            };
            let all_collision: Vec<[Vec3; 3]> = model_triangles(&mdl);
            let has_walkmesh = {
                let mut a = Vec::new();
                collect_triangles(&mdl.root_node, (Vec3::ZERO, Quat::IDENTITY), &mut a, true);
                !a.is_empty()
            };
            let drawn = rendered_triangles(&mdl);
            let coll: Vec<[Vec3; 3]> = o.triangles.iter().map(|t| t.map(|v| v - o.position)).collect();
            let hidden = coll
                .iter()
                .filter(|t| {
                    let c = (t[0] + t[1] + t[2]) / 3.0;
                    !drawn.iter().any(|d| closest_on_triangle(c, d[0], d[1], d[2]).distance(c) < 1.0)
                })
                .count();
            let gap = o.bounds.map_or(f32::MAX, |(lo, hi)| scenery_gap(lo, hi));
            // Drawn upright faces (walls of a crate or pillar) with their
            // centre in the collision box grown by 1.5 units, above the floor.
            let upright = o.bounds.map_or(0, |(lo, hi)| {
                scenery
                    .iter()
                    .filter(|t| {
                        let c = (t[0] + t[1] + t[2]) / 3.0;
                        let n = (t[1] - t[0]).cross(t[2] - t[0]).normalize_or_zero();
                        n.z.abs() < 0.7 && c.cmpge(lo - Vec3::new(1.5, 1.5, -2.0)).all() && c.cmple(hi + Vec3::splat(1.5)).all()
                    })
                    .count()
            });
            let near_scenery = if gap < 0.75 && upright > 0 { 1 } else { 0 };
            let flag = if drawn.is_empty() && !coll.is_empty() && near_scenery == 0 {
                "INVISIBLE"
            } else if drawn.is_empty() && !coll.is_empty() {
                "proxy of scenery"
            } else if hidden > 0 {
                "partly hidden"
            } else if coll.is_empty() {
                "no collision"
            } else {
                "ok"
            };
            if flag == "INVISIBLE"
                && let Some((lo, hi)) = o.bounds
            {
                let mid = (lo + hi) / 2.0;
                for e in &mg.enemies {
                    let n = 200;
                    let close = (0..=n).any(|i| e.track.sample(e.track.length * i as f32 / n as f32).0.distance(mid) < 12.0);
                    if close {
                        out.push(format!("    follower {} {:?} passes near {}", e.name, e.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), o.name));
                    }
                }
            }
            let mut aabb_names = Vec::new();
            fn names(n: &k2_mdl::MdlNode, out: &mut Vec<String>) {
                if matches!(n.node_data, k2_mdl::MdlNodeData::Aabb(_)) {
                    out.push(n.name.clone());
                }
                n.children.iter().for_each(|c| names(c, out));
            }
            names(&mdl.root_node, &mut aabb_names);
            let sc = &o.scripts;
            out.push(format!(
                "    obstacle {} walkmesh nodes {:?} scripts hitobstacle '{}' hitfollower '{}' animations {:?}",
                o.name,
                aabb_names,
                sc.on_hit_obstacle,
                sc.on_hit_follower,
                mdl.animations.iter().map(|a| a.name.as_str()).collect::<Vec<_>>()
            ));
            out.push(format!(
                "OBSTACLE {:<16} {flag:<16} scenery gap {gap:.1} upright faces {upright} walkmesh {has_walkmesh} drawn {} tris {} | collision {} of {} tris, {hidden} not near a drawn face | drawn {} collision {}",
                o.name,
                fmt(tri_bounds(&drawn)),
                drawn.len(),
                coll.len(),
                all_collision.len(),
                fmt(tri_bounds(&drawn)),
                fmt(tri_bounds(&coll))
            ));
        }
        for e in &mg.enemies {
            let mut drawn = 0;
            let mut missing = Vec::new();
            for m in &e.models {
                match load_mdl(self, &m.name) {
                    Some(mdl) => drawn += rendered_triangles(&mdl).len(),
                    None => missing.push(m.name.clone()),
                }
            }
            let flag = if !missing.is_empty() {
                "MODEL MISSING"
            } else if drawn == 0 && !e.trigger {
                "NOTHING DRAWN"
            } else if drawn == 0 {
                "trigger, nothing drawn"
            } else {
                "ok"
            };
            out.push(format!(
                "FOLLOWER {:<14} {flag:<22} models {:?} missing {:?} drawn tris {drawn} radius {:.1} trigger {} hit script '{}' track {:.0}s",
                e.name, e.models.iter().map(|m| m.name.as_str()).collect::<Vec<_>>(), missing, e.sphere_radius, e.trigger, e.scripts.on_hit_follower, e.track.length
            ));
        }
        out
    }

    /// SWMG_OnDeath: the die animation, the death sound, then gone.
    fn mg_default_death(&mut self, id: ObjectId) {
        let Some(mg) = self.minigame.as_mut() else { return };
        let Some(o) = mg.object_mut(id) else { return };
        o.alive = false;
        let (snd, pos) = (o.death_sound.clone(), o.position);
        mg.anim(id, "die", false);
        if !snd.is_empty() {
            mg.events.push(MgEvent::Sound { sound: snd, position: pos });
        }
    }

    /// Script helper for SWMG_* routines.
    pub(crate) fn minigame_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        if !name.starts_with("SWMG_") {
            return None;
        }
        let int = |i: usize| match a.get(i) {
            Some(Value::Int(v)) => *v,
            _ => 0,
        };
        let float = |i: usize| match a.get(i) {
            Some(Value::Float(v)) => *v,
            Some(Value::Int(v)) => *v as f32,
            _ => 0.0,
        };
        let obj = |i: usize| match a.get(i) {
            Some(Value::Object(v)) => *v,
            _ => me,
        };
        let string = |i: usize| match a.get(i) {
            Some(Value::String(s)) => s.to_string(),
            _ => String::new(),
        };
        let vec = |i: usize| match a.get(i) {
            Some(Value::Vector(v)) => Vec3::from(*v),
            _ => Vec3::ZERO,
        };
        let v = |p: Vec3| Value::Vector([p.x, p.y, p.z]);
        let o = Value::Object;
        let Some(mg) = self.minigame.as_mut() else {
            // Outside a minigame every SWMG call is a harmless no-op.
            let v = match name {
                "SWMG_GetPlayer" | "SWMG_GetEnemy" | "SWMG_GetObstacle" | "SWMG_GetLastFollowerHit" | "SWMG_GetLastObstacleHit"
                | "SWMG_GetLastBulletHitShooter" | "SWMG_GetObjectByName" => o(OBJECT_INVALID),
                _ => Value::Void,
            };
            return Some(default_like(name, v));
        };
        let r = match name {
            "SWMG_GetPlayer" => o(mg.player.id),
            "SWMG_GetEnemyCount" => Value::Int(mg.enemies.len() as i32),
            "SWMG_GetEnemy" => o(mg.enemies.get(int(0).max(0) as usize).map_or(OBJECT_INVALID, |e| e.id)),
            "SWMG_GetObstacleCount" => Value::Int(mg.obstacles.len() as i32),
            "SWMG_GetObstacle" => o(mg.obstacles.get(int(0).max(0) as usize).map_or(OBJECT_INVALID, |e| e.id)),
            "SWMG_GetObjectByName" => {
                let n = string(0).to_ascii_lowercase();
                o(std::iter::once(&mg.player).chain(&mg.enemies).chain(&mg.obstacles).find(|x| x.name == n).map_or(OBJECT_INVALID, |x| x.id))
            }
            "SWMG_GetObjectName" => Value::String(mg.object(obj(0)).map_or(String::new(), |x| x.name.clone()).into()),
            "SWMG_IsPlayer" => Value::Int(i32::from(obj(0) == mg.player.id)),
            "SWMG_IsEnemy" => Value::Int(i32::from(mg.enemies.iter().any(|e| e.id == obj(0)))),
            "SWMG_IsFollower" => Value::Int(i32::from(obj(0) == mg.player.id || mg.enemies.iter().any(|e| e.id == obj(0)))),
            "SWMG_IsTrigger" => Value::Int(i32::from(mg.enemies.iter().any(|e| e.id == obj(0) && e.trigger))),
            "SWMG_IsObstacle" => Value::Int(i32::from(mg.obstacles.iter().any(|e| e.id == obj(0)))),
            "SWMG_PlayAnimation" => {
                let looping = a.len() < 3 || int(2) != 0;
                mg.events.push(MgEvent::Animation { object: obj(0), anim: string(1).to_ascii_lowercase(), looping, queue: int(3) != 0, overlay: int(4) != 0 });
                Value::Void
            }
            "SWMG_RemoveAnimation" => {
                mg.events.push(MgEvent::RemoveAnimation { object: obj(0), anim: string(1).to_ascii_lowercase() });
                Value::Void
            }
            "SWMG_GetPosition" => v(mg.object(obj(0)).map_or(Vec3::ZERO, |x| x.position)),
            "SWMG_GetTrackPosition" => v(mg.object(obj(0)).map_or(Vec3::ZERO, |x| x.track.sample(x.track_time).0)),
            "SWMG_SetFollowerPosition" => {
                // Moves the calling follower along its track to the closest point.
                let p = vec(0);
                if let Some(x) = mg.object_mut(me)
                    && x.track.length > 0.0
                {
                    let mut best = (f32::MAX, x.track_time);
                    for i in 0..=100 {
                        let t = x.track.length * i as f32 / 100.0;
                        let d = x.track.sample(t).0.distance(p);
                        if d < best.0 {
                            best = (d, t);
                        }
                    }
                    x.track_time = best.1;
                }
                v(p)
            }
            "SWMG_GetHitPoints" => Value::Int(mg.object(obj(0)).map_or(0, |x| x.hp)),
            "SWMG_GetMaxHitPoints" => Value::Int(mg.object(obj(0)).map_or(0, |x| x.max_hp)),
            "SWMG_SetMaxHitPoints" => {
                if let Some(x) = mg.object_mut(obj(0)) {
                    x.max_hp = int(1).max(1);
                }
                Value::Void
            }
            "SWMG_SetFollowerHitPoints" => {
                let id = obj(0);
                if let Some(x) = mg.object_mut(id) {
                    let old = x.hp;
                    x.hp = int(1);
                    mg.last_hp_change = int(1) - old;
                }
                Value::Void
            }
            "SWMG_AdjustFollowerHitPoints" => {
                let (id, hp, absolute) = (obj(0), int(1), int(2) != 0);
                let mut out = 0;
                if let Some(x) = mg.object_mut(id) {
                    let old = x.hp;
                    x.hp = if absolute { hp } else { x.hp + hp };
                    x.hp = x.hp.min(x.max_hp);
                    out = x.hp;
                    mg.last_hp_change = x.hp - old;
                }
                Value::Int(out)
            }
            "SWMG_GetLastHPChange" => Value::Int(mg.last_hp_change),
            "SWMG_GetSphereRadius" => Value::Float(mg.object(obj(0)).map_or(0.0, |x| x.sphere_radius)),
            "SWMG_SetSphereRadius" => {
                if let Some(x) = mg.object_mut(obj(0)) {
                    x.sphere_radius = float(1).max(0.0);
                }
                Value::Void
            }
            "SWMG_GetNumLoops" => Value::Int(mg.object(obj(0)).map_or(0, |x| x.loops)),
            "SWMG_SetNumLoops" => {
                if let Some(x) = mg.object_mut(obj(0)) {
                    x.loops = int(1);
                }
                Value::Void
            }
            "SWMG_GetIsInvulnerable" => Value::Int(i32::from(mg.object(obj(0)).is_some_and(|x| x.invulnerable_for > 0.0))),
            "SWMG_StartInvulnerability" => {
                if let Some(x) = mg.object_mut(obj(0)) {
                    x.invulnerable_for = x.invince_period.max(0.5);
                }
                Value::Void
            }
            "SWMG_OnDeath" => {
                self.mg_default_death(me);
                return Some(Value::Void);
            }
            "SWMG_OnDamage" => {
                mg.anim(me, "damage", false);
                Value::Void
            }
            "SWMG_OnBulletHit" | "SWMG_OnObstacleHit" => Value::Void,
            "SWMG_DestroyMiniGameObject" => {
                let id = obj(0);
                if let Some(x) = mg.object_mut(id) {
                    x.removed = true;
                    x.alive = false;
                    mg.events.push(MgEvent::Removed { object: id });
                }
                Value::Void
            }
            "SWMG_GetLastFollowerHit" => o(mg.last_follower_hit),
            "SWMG_GetLastObstacleHit" => o(mg.last_obstacle_hit),
            "SWMG_GetLastBulletHitDamage" => Value::Int(mg.last_bullet_hit_damage),
            "SWMG_GetLastBulletHitTarget" => Value::Int(mg.last_bullet_hit_target),
            "SWMG_GetLastBulletHitShooter" => o(mg.last_bullet_hit_shooter),
            "SWMG_GetLastBulletFiredDamage" => Value::Int(mg.last_bullet_fired_damage),
            "SWMG_GetLastBulletFiredTarget" => Value::Int(mg.last_bullet_fired_target),
            "SWMG_GetLastBulletHitPart" => Value::String("".into()),
            "SWMG_GetLastEvent" => Value::String(mg.last_event.clone().into()),
            "SWMG_GetLastEventModelName" => Value::String(mg.last_event_model.clone().into()),
            "SWMG_GetCameraNearClip" => Value::Float(mg.near_clip),
            "SWMG_GetCameraFarClip" => Value::Float(mg.far_clip),
            "SWMG_SetCameraClip" => {
                mg.near_clip = float(0);
                mg.far_clip = float(1);
                Value::Void
            }
            "SWMG_GetPlayerOffset" => v(mg.offset),
            "SWMG_SetPlayerOffset" => {
                mg.offset = vec(0);
                Value::Void
            }
            "SWMG_GetPlayerOrigin" => v(mg.origin.unwrap_or(mg.player.position)),
            "SWMG_SetPlayerOrigin" => {
                mg.origin = Some(vec(0));
                Value::Void
            }
            "SWMG_GetPlayerInvincibility" => Value::Float(mg.invincibility),
            "SWMG_SetPlayerInvincibility" => {
                mg.invincibility = float(0);
                Value::Void
            }
            "SWMG_GetPlayerSpeed" => Value::Float(mg.speed),
            "SWMG_SetPlayerSpeed" => {
                mg.speed = float(0);
                Value::Void
            }
            "SWMG_GetPlayerMinSpeed" => Value::Float(mg.min_speed),
            "SWMG_SetPlayerMinSpeed" => {
                mg.min_speed = float(0);
                Value::Void
            }
            "SWMG_GetPlayerMaxSpeed" => Value::Float(mg.max_speed),
            "SWMG_SetPlayerMaxSpeed" => {
                mg.max_speed = float(0);
                Value::Void
            }
            "SWMG_GetPlayerAccelerationPerSecond" => Value::Float(mg.accel_per_sec),
            "SWMG_SetPlayerAccelerationPerSecond" => {
                mg.accel_per_sec = float(0);
                Value::Void
            }
            "SWMG_GetLateralAccelerationPerSecond" => Value::Float(mg.lateral_aps),
            "SWMG_SetLateralAccelerationPerSecond" => {
                mg.lateral_aps = float(0);
                Value::Void
            }
            "SWMG_GetPlayerTunnelPos" => v(mg.tunnel_pos),
            "SWMG_SetPlayerTunnelPos" => {
                mg.tunnel_pos = vec(0);
                Value::Void
            }
            "SWMG_GetPlayerTunnelNeg" => v(mg.tunnel_neg),
            "SWMG_SetPlayerTunnelNeg" => {
                mg.tunnel_neg = vec(0);
                Value::Void
            }
            "SWMG_GetPlayerTunnelInfinite" => v(mg.tunnel_infinite),
            "SWMG_SetPlayerTunnelInfinite" => {
                mg.tunnel_infinite = vec(0);
                Value::Void
            }
            "SWMG_SetJumpSpeed" => {
                if mg.offset.z <= 0.01 {
                    mg.vertical_vel = float(0);
                }
                Value::Void
            }
            "SWMG_PlayerApplyForce" => {
                let f = vec(0);
                mg.lateral_vel += f.x;
                mg.vertical_vel += f.z;
                mg.speed = (mg.speed + f.y).max(0.0);
                Value::Void
            }
            "SWMG_SetSpeedBlurEffect" => {
                let enabled = int(0) != 0;
                let ratio = if a.len() > 1 { float(1) } else { 0.75 };
                mg.events.push(MgEvent::SpeedBlur { enabled, ratio });
                Value::Void
            }
            "SWMG_GetSwoopUpgrade" => {
                // The purchase screen (ShowSwoopUpgradeScreen, called by
                // k_ptar_swoopup) sets no global or item that any shipped script
                // names, and no save shows one. Storage not found, so 0 for
                // every group (docs/notes/minigames.md, "Swoop upgrades").
                Value::Int(0)
            }
            "SWMG_GetGunBankCount" => Value::Int(mg.object(obj(0)).map_or(0, |x| x.gun_banks.len() as i32)),
            "SWMG_IsGunBankTargetting" => Value::Int(i32::from(
                mg.object(obj(0)).and_then(|x| x.gun_banks.get(int(1).max(0) as usize)).is_some_and(|g| g.target_type != 0),
            )),
            n if n.starts_with("SWMG_GetGunBank") || n.starts_with("SWMG_SetGunBank") => {
                let (id, bi) = (obj(0), int(1).max(0) as usize);
                let Some(g) = mg.object_mut(id).and_then(|x| x.gun_banks.get_mut(bi)) else { return Some(default_like(name, Value::Void)) };
                match &n[5..] {
                    "GetGunBankBulletModel" => Value::String(g.bullet_model.clone().into()),
                    "GetGunBankGunModel" => Value::String(g.gun_model.clone().into()),
                    "GetGunBankDamage" => Value::Int(g.damage),
                    "GetGunBankTimeBetweenShots" => Value::Float(g.rate_of_fire),
                    "GetGunBankLifespan" => Value::Float(g.lifespan),
                    "GetGunBankSpeed" => Value::Float(g.speed),
                    "GetGunBankTarget" => Value::Int(g.target_type as i32),
                    "GetGunBankHorizontalSpread" => Value::Float(g.horiz_spread),
                    "GetGunBankVerticalSpread" => Value::Float(g.vert_spread),
                    "GetGunBankSensingRadius" => Value::Float(g.sensing_radius),
                    "GetGunBankInaccuracy" => Value::Float(g.inaccuracy),
                    "SetGunBankBulletModel" => {
                        g.bullet_model = string(2).to_ascii_lowercase();
                        Value::Void
                    }
                    "SetGunBankGunModel" => {
                        g.gun_model = string(2).to_ascii_lowercase();
                        Value::Void
                    }
                    "SetGunBankDamage" => {
                        g.damage = int(2);
                        Value::Void
                    }
                    "SetGunBankTimeBetweenShots" => {
                        g.rate_of_fire = float(2);
                        Value::Void
                    }
                    "SetGunBankLifespan" => {
                        g.lifespan = float(2);
                        Value::Void
                    }
                    "SetGunBankSpeed" => {
                        g.speed = float(2);
                        Value::Void
                    }
                    "SetGunBankTarget" => {
                        g.target_type = int(2).max(0) as u32;
                        Value::Void
                    }
                    "SetGunBankHorizontalSpread" => {
                        g.horiz_spread = float(2);
                        Value::Void
                    }
                    "SetGunBankVerticalSpread" => {
                        g.vert_spread = float(2);
                        Value::Void
                    }
                    "SetGunBankSensingRadius" => {
                        g.sensing_radius = float(2);
                        Value::Void
                    }
                    "SetGunBankInaccuracy" => {
                        g.inaccuracy = float(2);
                        Value::Void
                    }
                    _ => default_like(name, Value::Void),
                }
            }
            // Per-follower sound tweaks (frequency/volume) have no audible
            // model here; reads return neutral values.
            n if n.starts_with("SWMG_GetSound") => Value::Int(0),
            n if n.starts_with("SWMG_SetSound") => Value::Void,
            _ => default_like(name, Value::Void),
        };
        Some(r)
    }
}

/// Fallback values by the routine's name pattern (the VM checks types).
fn default_like(name: &str, v: Value) -> Value {
    if !matches!(v, Value::Void) {
        return v;
    }
    let n = name.trim_start_matches("SWMG_");
    if n.starts_with("Get") || n.starts_with("Is") {
        if n.contains("Position") || n.contains("Offset") || n.contains("Tunnel") || n.contains("Origin") {
            return Value::Vector([0.0; 3]);
        }
        if n.contains("Speed") || n.contains("Radius") || n.contains("Clip") || n.contains("Spread") || n.contains("Invincibility") {
            return Value::Float(0.0);
        }
        if n.contains("Name") || n.contains("Model") || n.contains("Event") || n.contains("Part") {
            return Value::String("".into());
        }
        return Value::Int(0);
    }
    v
}

fn clamp_offset(mg: &mut MiniGame) {
    let (p, n, inf) = (mg.tunnel_pos, mg.tunnel_neg, mg.tunnel_infinite);
    let axis = |v: &mut f32, pos: f32, neg: f32, infinite: f32, angles: bool| {
        if infinite != 0.0 {
            if angles {
                *v = v.rem_euclid(360.0);
            }
            return;
        }
        if pos == 0.0 && neg == 0.0 {
            if !angles {
                *v = 0.0;
            }
            return;
        }
        *v = v.clamp(-neg.abs(), pos.abs());
    };
    let angles = mg.is_turret();
    axis(&mut mg.offset.x, p.x, n.x, inf.x, angles);
    axis(&mut mg.offset.y, p.y, n.y, inf.y, angles);
    if !angles {
        // Swoop height is jump-driven; only an explicit ceiling limits it.
        if p.z != 0.0 {
            mg.offset.z = mg.offset.z.min(p.z.abs());
        }
    } else {
        axis(&mut mg.offset.z, p.z, n.z, inf.z, angles);
    }
}

/// Whether segment a-b passes within r of c.
fn segment_sphere(a: Vec3, b: Vec3, c: Vec3, r: f32) -> bool {
    let ab = b - a;
    let t = if ab.length_squared() > 0.0 { ((c - a).dot(ab) / ab.length_squared()).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(c) <= r
}

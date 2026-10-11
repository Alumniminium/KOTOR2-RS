//! Animated creatures: supermodel animation lookup, per-node transform
//! sampling with crossfades, skinned body + head assembly.
//!
//! Animations are found by name in the model, then its supermodel, and so on
//! (e.g. pmbam -> S_Female02 -> S_Female01 -> S_Male02 -> S_Male01). Tracks
//! bind to the creature's nodes by name.

pub mod clip;
pub mod lipsync;
pub mod viewer;

use std::sync::Arc;

use bevy::prelude::*;
use bevy::transform::TransformSystems;

use crate::material::{Blend, FLAG_ALPHA_MASK, KMaterial};
use crate::model::{Loader, ModelTemplate, spawn_tree};
use clip::Clip;

pub struct AnimPlugin;

impl Plugin for AnimPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(PostUpdate, (animate.in_set(AnimSystems), animate_alpha.after(animate)).before(TransformSystems::Propagate));
    }
}

/// A model and its supermodel chain, nearest first.
pub struct AnimSet {
    chain: Vec<Arc<ModelTemplate>>,
}

impl AnimSet {
    pub fn resolve(loader: &mut Loader, model: &str) -> Option<Arc<AnimSet>> {
        let mut chain = vec![loader.model(model)?];
        while chain.len() < 12 {
            let Some(sup) = chain.last().unwrap().supermodel.clone() else { break };
            match loader.model(&sup) {
                Some(t) => chain.push(t),
                None => break,
            }
        }
        Some(Arc::new(AnimSet { chain }))
    }

    pub fn clip(&self, name: &str) -> Option<Arc<Clip>> {
        let name = name.to_ascii_lowercase();
        self.chain.iter().find_map(|t| t.clips.get(&name).cloned())
    }

    #[allow(dead_code)] // gameplay API
    pub fn has(&self, name: &str) -> bool {
        self.clip(name).is_some()
    }

    /// The model's own looping pose, ignoring the supermodel chain: "default",
    /// else the first own clip named like a loop (`cut001l`). Stunt
    /// placeables (a creature skeleton posed for a scene, e.g. p_kreiastunt)
    /// carry such clips and inherit a standing pause1 from the creature
    /// supermodel, which must not win.
    pub fn own_loop(&self) -> Option<Arc<Clip>> {
        let own = self.chain.first()?;
        if let Some(c) = own.clips.get("default") {
            return Some(c.clone());
        }
        let mut names: Vec<&String> = own.clips.keys().filter(|n| n.ends_with('l')).collect();
        names.sort();
        names.first().and_then(|n| own.clips.get(*n).cloned())
    }

    /// All animation names reachable through the chain, nearest first.
    pub fn names(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in &self.chain {
            let mut names: Vec<&String> = t.clips.keys().collect();
            names.sort();
            for n in names {
                if !out.contains(n) {
                    out.push(n.clone());
                }
            }
        }
        out
    }
}

#[derive(Clone)]
struct Playing {
    clip: Arc<Clip>,
    time: f32,
    looping: bool,
    speed: f32,
    done: bool,
}

impl Playing {
    /// Advances the clock and appends the names of clip events crossed in
    /// (old, new] to `fired` (both halves when a loop wraps). An event at
    /// exactly 0 fires on the first advance of a fresh clip.
    fn advance(&mut self, dt: f32, fired: &mut Vec<String>) {
        if self.done {
            return;
        }
        let old = self.time;
        let fresh = old == 0.0;
        self.time += dt * self.speed;
        let len = self.clip.length;
        let raw = self.time;
        if len <= 0.0 {
            self.time = 0.0;
            self.done = !self.looping;
        } else if self.looping {
            self.time = self.time.rem_euclid(len);
        } else if self.time >= len {
            self.time = len;
            self.done = true;
        }
        if self.clip.events.is_empty() || len <= 0.0 || fired.len() >= MAX_PENDING_EVENTS {
            return;
        }
        let crossed = |a: f32, b: f32, t: f32| (t > a || (fresh && t == 0.0 && a == 0.0)) && t <= b;
        for (t, name) in &self.clip.events {
            let hit = if self.looping && raw >= len {
                crossed(old, len, *t) || *t <= self.time
            } else {
                crossed(old, raw.min(len), *t)
            };
            if hit {
                fired.push(name.clone());
            }
        }
    }
}

/// Events kept for a consumer between drains; older ones are dropped.
const MAX_PENDING_EVENTS: usize = 32;

/// Plays MDL animations on a creature. Lives on the creature root alongside
/// `AnimBindings`.
#[derive(Component)]
pub struct AnimPlayer {
    pub set: Arc<AnimSet>,
    cur: Option<Playing>,
    prev: Option<Playing>,
    fade: f32,
    fade_len: f32,
    /// MDL clip events (lowercase, e.g. "hit", "swingshort", "snd_footstep")
    /// crossed by the current clip since the last `drain_events`.
    events: Vec<String>,
}

impl AnimPlayer {
    pub fn new(set: Arc<AnimSet>) -> Self {
        Self { set, cur: None, prev: None, fade: 0.0, fade_len: 0.0, events: Vec::new() }
    }

    /// Switches immediately. Calling again with the clip already playing (and
    /// not finished) does nothing, so it is safe to call every frame.
    pub fn play(&mut self, name: &str, looping: bool, speed: f32) -> bool {
        self.fade_to(name, looping, speed, 0.0)
    }

    /// Crossfades from the current pose over `fade` seconds.
    pub fn fade_to(&mut self, name: &str, looping: bool, speed: f32, fade: f32) -> bool {
        let name = name.to_ascii_lowercase();
        if let Some(c) = &mut self.cur {
            if c.clip.name == name && c.looping == looping && !c.done {
                c.speed = speed;
                return true;
            }
        }
        let Some(clip) = self.set.clip(&name) else { return false };
        let next = Playing { clip, time: 0.0, looping, speed, done: false };
        if fade > 0.0 && self.cur.is_some() {
            self.prev = self.cur.take();
            self.fade = 0.0;
            self.fade_len = fade;
        } else {
            self.prev = None;
        }
        self.cur = Some(next);
        true
    }

    #[allow(dead_code)] // gameplay API
    pub fn current(&self) -> Option<&str> {
        self.cur.as_ref().map(|c| c.clip.name.as_str())
    }

    /// True when a one-shot clip reached its end (or nothing is playing).
    #[allow(dead_code)] // gameplay API
    pub fn finished(&self) -> bool {
        self.cur.as_ref().is_none_or(|c| c.done)
    }

    /// The playing clip and its time (emitters read keyed birthrates and
    /// clip events from it).
    pub fn current_clip(&self) -> Option<(&Arc<Clip>, f32)> {
        self.cur.as_ref().map(|c| (&c.clip, c.time))
    }

    /// Normalized progress of the current clip, 0..1.
    #[allow(dead_code)] // gameplay API
    pub fn progress(&self) -> f32 {
        self.cur.as_ref().map_or(1.0, |c| if c.clip.length > 0.0 { c.time / c.clip.length } else { 1.0 })
    }

    /// Clip events fired since the last call, oldest first.
    pub fn drain_events(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events)
    }

    /// Event times of the current clip (seconds), for callers that need to
    /// know whether a clip carries a given event at all.
    pub fn current_events(&self) -> &[(f32, String)] {
        self.cur.as_ref().map_or(&[], |c| c.clip.events.as_slice())
    }

    /// Node alpha of the current clip (crossfaded) for a lowercased node
    /// name; 1 when the clip has no alpha track for it.
    pub fn sample_alpha(&self, key: &str) -> f32 {
        let of = |p: &Playing| p.clip.tracks.get(key).and_then(|t| t.sample_alpha(p.time)).unwrap_or(1.0);
        let Some(cur) = &self.cur else { return 1.0 };
        let a = of(cur);
        match &self.prev {
            Some(prev) if self.fade_len > 0.0 => {
                let w = (self.fade / self.fade_len).clamp(0.0, 1.0);
                of(prev) + (a - of(prev)) * w
            }
            _ => a,
        }
    }

    /// Jumps the current clip to `time` seconds (used by the viewer).
    pub fn seek(&mut self, time: f32) {
        if let Some(c) = &mut self.cur {
            c.time = time.clamp(0.0, c.clip.length.max(0.0));
        }
    }
}

pub struct Bound {
    key: String,
    entity: Entity,
    rest_t: Vec3,
    rest_r: Quat,
}

/// The system that poses nodes from AnimPlayer (PostUpdate). Overlay
/// animations (minigame HUDs) run after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct AnimSystems;

/// Animated node entities of a creature, with rest poses.
#[derive(Component)]
pub struct AnimBindings {
    pub nodes: Vec<Bound>,
    pub anim_scale: f32,
}

impl AnimBindings {
    #[allow(dead_code)] // gameplay API
    pub fn entity(&self, node: &str) -> Option<Entity> {
        let node = node.to_ascii_lowercase();
        self.nodes.iter().find(|b| b.key == node).map(|b| b.entity)
    }

    /// Bound nodes as (lowercase name, entity, rest translation, rest rotation).
    pub fn rest_poses(&self) -> impl Iterator<Item = (&str, Entity, Vec3, Quat)> {
        self.nodes.iter().map(|b| (b.key.as_str(), b.entity, b.rest_t, b.rest_r))
    }
}

fn pose(p: &Playing, b: &Bound, scale: f32) -> (Vec3, Quat, f32) {
    match p.clip.tracks.get(&b.key) {
        Some(tr) => (
            tr.sample_pos(p.time).map_or(b.rest_t, |d| b.rest_t + d * scale),
            tr.sample_rot(p.time).unwrap_or(b.rest_r),
            tr.sample_scale(p.time).unwrap_or(1.0),
        ),
        None => (b.rest_t, b.rest_r, 1.0),
    }
}

fn animate(time: Res<Time>, mut players: Query<(&mut AnimPlayer, &AnimBindings)>, mut transforms: Query<&mut Transform>) {
    let dt = time.delta_secs();
    for (mut player, bindings) in &mut players {
        let player = &mut *player;
        if let Some(c) = &mut player.cur {
            c.advance(dt, &mut player.events);
        }
        if let Some(p) = &mut player.prev {
            // The outgoing clip of a crossfade is silent.
            p.advance(dt, &mut Vec::new());
            player.fade += dt;
            if player.fade >= player.fade_len {
                player.prev = None;
            }
        }
        let Some(cur) = &player.cur else { continue };
        let w = if player.fade_len > 0.0 { (player.fade / player.fade_len).clamp(0.0, 1.0) } else { 1.0 };
        for b in &bindings.nodes {
            let (mut t, mut r, mut s) = pose(cur, b, bindings.anim_scale);
            if let Some(prev) = &player.prev {
                let (t0, r0, s0) = pose(prev, b, bindings.anim_scale);
                t = t0.lerp(t, w);
                r = r0.slerp(r, w);
                s = s0 + (s - s0) * w;
            }
            if let Ok(mut tf) = transforms.get_mut(b.entity) {
                tf.translation = t;
                tf.rotation = r;
                tf.scale = Vec3::splat(s);
            }
        }
    }
}

/// Mesh nodes of a model whose clips animate alpha (force cages, console
/// screens, holograms). Each owns private materials so one node can fade
/// without its siblings; `animate_alpha` writes the sampled value into them.
#[derive(Component)]
pub struct AlphaNodes(Vec<AlphaNode>);

struct AlphaNode {
    key: String,
    handles: Vec<Handle<KMaterial>>,
    last: f32,
}

/// Node alpha set by overlay clips (the minigame swoop's `damage` shield
/// flicker), by lowercase node name; wins over the main clip's alpha.
#[derive(Component, Default)]
pub struct AlphaOverlay(pub std::collections::HashMap<String, f32>);

fn animate_alpha(mut players: Query<(&AnimPlayer, &mut AlphaNodes, Option<&AlphaOverlay>)>, mut materials: ResMut<Assets<KMaterial>>) {
    for (player, mut an, over) in &mut players {
        for node in &mut an.0 {
            let a = over.and_then(|o| o.0.get(&node.key).copied()).unwrap_or_else(|| player.sample_alpha(&node.key));
            if (a - node.last).abs() < 0.004 {
                continue;
            }
            node.last = a;
            for h in &node.handles {
                if let Some(mut m) = materials.get_mut(h) {
                    m.params.tint.alpha = a;
                }
            }
        }
    }
}

/// Idle clips tried in order when a model is spawned.
pub const IDLE_ANIMS: [&str; 4] = ["pause1", "cpause1", "default", "closed"];

/// What to spawn for a creature.
#[derive(Clone, Debug, Default)]
pub struct CreatureSpec {
    /// Body model (appearance.2da modela/modelb).
    pub body: String,
    /// Head model attached at the body's `headhook` (heads.2da head), if any.
    pub head: Option<String>,
    /// Body texture override, e.g. appearance texa + "01".
    pub texture: Option<String>,
    /// Prefer the model's own loop clip over the inherited idles (placeables).
    pub own_idle: bool,
}

/// Spawns a creature under `parent` with `AnimPlayer` + `AnimBindings` on the
/// returned root. Starts in pause1 when the chain has it.
pub fn spawn_creature(
    commands: &mut Commands,
    loader: &mut Loader,
    spec: &CreatureSpec,
    parent: Entity,
    transform: Transform,
) -> Option<Entity> {
    let body = loader.model(&spec.body)?;
    let set = AnimSet::resolve(loader, &spec.body)?;
    let material: Option<Handle<KMaterial>> = spec.texture.as_deref().filter(|t| loader.texture(t).is_some()).map(|t| loader.texture_material(t));
    let nodes = spawn_tree(commands, &body, parent, transform, material.as_ref());
    let root = nodes[0];

    let flat = body.flat();
    let mut bound: Vec<Bound> = flat
        .iter()
        .zip(&nodes)
        .skip(1) // the model root keeps the placement transform
        .map(|(n, &e)| Bound { key: n.name.to_ascii_lowercase(), entity: e, rest_t: n.translation, rest_r: n.rotation })
        .collect();

    if let Some(head_name) = spec.head.as_deref() {
        let hook = flat.iter().position(|n| n.name.eq_ignore_ascii_case("headhook"));
        match (loader.model(head_name), hook) {
            (Some(head), Some(hook)) => {
                let hn = spawn_tree(commands, &head, nodes[hook], Transform::IDENTITY, None);
                let hflat = head.flat();
                // Only the head's animated subtree (anim root, usually neck_g)
                // takes body animation tracks; the rest is the attachment.
                let start = head
                    .anim_root
                    .as_deref()
                    .and_then(|r| hflat.iter().position(|n| n.name.eq_ignore_ascii_case(r)))
                    .unwrap_or(1);
                let in_subtree = subtree_mask(&head, start);
                for (i, (n, &e)) in hflat.iter().zip(&hn).enumerate() {
                    if in_subtree[i] {
                        bound.push(Bound { key: n.name.to_ascii_lowercase(), entity: e, rest_t: n.translation, rest_r: n.rotation });
                    }
                }
            }
            (None, _) => warn!("head model {head_name} missing"),
            (_, None) => warn!("body {} has no headhook", spec.body),
        }
    }

    queue_alpha_nodes(commands, &body, &flat, &nodes);

    let own = if spec.own_idle { set.own_loop() } else { None };
    let mut player = AnimPlayer::new(set);
    // Idle: humanoids pause1, simple creatures (droids) cpause1, placeables
    // default, doors closed.
    if !own.is_some_and(|c| player.play(&c.name.to_ascii_lowercase(), true, 1.0)) {
        for idle in IDLE_ANIMS {
            if player.play(idle, true, 1.0) {
                break;
            }
        }
    }
    commands.entity(root).insert((player, AnimBindings { nodes: bound, anim_scale: body.anim_scale }));
    Some(root)
}

/// Gives every mesh node whose own clips fade alpha below 1 private
/// materials (cloned from what it spawned with, blended when it was opaque)
/// and records them as `AlphaNodes` on the model root.
fn queue_alpha_nodes(commands: &mut Commands, body: &ModelTemplate, flat: &[&crate::model::TNode], nodes: &[Entity]) {
    let mut min_alpha: std::collections::HashMap<&str, f32> = std::collections::HashMap::new();
    for clip in body.clips.values() {
        for (key, track) in &clip.tracks {
            for &(_, a) in &track.alpha {
                let e = min_alpha.entry(key.as_str()).or_insert(1.0);
                *e = e.min(a);
            }
        }
    }
    let targets: Vec<(String, Entity)> = flat
        .iter()
        .zip(nodes)
        .filter(|(n, _)| !n.meshes.is_empty())
        .filter_map(|(n, &e)| {
            let key = n.name.to_ascii_lowercase();
            (min_alpha.get(key.as_str()).copied().unwrap_or(1.0) < 0.995).then_some((key, e))
        })
        .collect();
    if targets.is_empty() {
        return;
    }
    let root = nodes[0];
    commands.queue(move |world: &mut World| {
        let mut out = Vec::new();
        for (key, node) in targets {
            let children: Vec<Entity> = world.get::<Children>(node).map(|c| c.iter().collect()).unwrap_or_default();
            let mut handles = Vec::new();
            for ch in children {
                let Some(old) = world.get::<MeshMaterial3d<KMaterial>>(ch).map(|m| m.0.clone()) else { continue };
                let Some(mut mat) = world.resource::<Assets<KMaterial>>().get(&old).cloned() else { continue };
                if mat.blend == Blend::Opaque {
                    mat.blend = Blend::Alpha;
                    mat.params.flags |= FLAG_ALPHA_MASK;
                    mat.params.alpha_cut = 0.02;
                }
                let h = world.resource_mut::<Assets<KMaterial>>().add(mat);
                world.entity_mut(ch).insert(MeshMaterial3d(h.clone()));
                handles.push(h);
            }
            if !handles.is_empty() {
                out.push(AlphaNode { key, handles, last: -1.0 });
            }
        }
        if !out.is_empty() {
            if let Ok(mut r) = world.get_entity_mut(root) {
                r.insert(AlphaNodes(out));
            }
        }
    });
}

/// Marks the pre-order indices inside the subtree rooted at `start`.
fn subtree_mask(t: &ModelTemplate, start: usize) -> Vec<bool> {
    fn go(n: &crate::model::TNode, idx: &mut usize, start: usize, inside: bool, out: &mut Vec<bool>) {
        let me = *idx;
        let inside = inside || me == start;
        out.push(inside);
        *idx += 1;
        for c in &n.children {
            go(c, idx, start, inside, out);
        }
    }
    let mut out = Vec::new();
    go(&t.root, &mut 0, start, false, &mut out);
    out
}

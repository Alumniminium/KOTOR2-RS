//! Projectiles in flight (k2-world `WorldEvent::Projectile`): blaster bolts
//! from ranged attacks, and grenades, rockets and darts from spells.
//!
//! The world decides when a projectile leaves, how long it flies and how it
//! ends. This module only draws it:
//! - Bolt (from the `rhand` or `lhand` the world names, and the `bullethook` of
//!   the weapon held there): two crossed additive streaks in the bolt model's
//!   emitter texture, lit by its light node's colour, with the muzzle flash
//!   model (ammunitiontypes `muzzleflash`, carried by the event) at the
//!   muzzle. The travel emitter of the bolt model is burst-only, so it is not
//!   run; the streak stands in for it.
//! - Thrown or launched item (hook `rhand`): its item model (spells.2da
//!   `projmodel`). A grenade is lobbed in an arc and tumbles; a rocket or dart
//!   flies straight.
//! - Landing: a flash on a hit or a deflection (a deflected bolt bounces back
//!   toward the shooter); an explosion (visualeffects model, its `impact` clip
//!   and emitters) and sound for a detonation; a miss flies on and is gone.
//!
//! Launch times and flights are measured on the virtual clock (`Time` in
//! Update), so a pause holds them.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;

use bevy::ecs::system::SystemParam;
use bevy::math::primitives::{Rectangle, Sphere};
use bevy::prelude::*;
use k2_mdl::{MdlControllerType, MdlNode, MdlNodeData};
use k2_res::{ResKey, Resources};
use k2_script::ObjectId;
use k2_world::WorldEvent;
use k2_world::projectiles::ProjectileEnd;
use rakata_core::ResourceType;

use crate::anim::{AnimBindings, AnimPlayer, CreatureSpec, spawn_creature};
use crate::audio::Voice;
use crate::coords::to_bevy;
use crate::emitter::FxLive;
use crate::game::{ApplyEvents, FrameEvents, GameSet, ObjBody, ObjEntities};
use crate::material::{Blend, FLAG_ADDITIVE, KMaterial};
use crate::model::{AssetCache, GameRes, Loader, spawn_tree};

/// Streak size in metres: thin, and a little longer than a frame of travel.
const STREAK_WIDTH: f32 = 0.12;
const STREAK_LENGTH: f32 = 1.2;
/// Light carried by a bolt. Untuned.
const BOLT_LIGHT: f32 = 4_000.0;
const BOLT_LIGHT_RANGE: f32 = 3.0;
/// Impact flash: seconds, and the sphere's radius before it grows.
const FLASH_LIFE: f32 = 0.25;
const FLASH_RADIUS: f32 = 0.3;
/// An explosion's emitters are created when its impact clip runs; the root
/// is kept this long, and the particles live on their own lifetimes.
const BLAST_LIFE: f32 = 12.0;
/// Sound range of an explosion (min full volume, max silent), as sfx.rs.
const BLAST_RANGE: (f32, f32) = (4.0, 40.0);
/// A deflected bolt bounces this far back toward the shooter, in this time.
const BOUNCE_LENGTH: f32 = 12.0;
const BOUNCE_TIME: f32 = 0.4;
/// Height of a shooter's hook above its feet, as k2-world's HOOK_HEIGHT (m).
const HOOK_HEIGHT: f32 = 1.1;

pub struct ProjectilesPlugin;

impl Plugin for ProjectilesPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Queued>()
            .init_resource::<Landed>()
            .init_resource::<FxCache>()
            .add_systems(Update, (reset_on_module, queue, launch, fly, dress, land, start_clips, grow_bursts, expire).chain().in_set(GameSet::Sync).after(ApplyEvents));
    }
}

/// A projectile the world has launched, waiting for its launch time.
struct Due {
    at: f32,
    shooter: ObjectId,
    model: String,
    /// The hand it leaves from (`rhand` or `lhand`).
    hand: String,
    bolt: bool,
    muzzle: Option<String>,
    /// Bevy space.
    to: Vec3,
    flight: f32,
    arc: bool,
    end: ProjectileEnd,
    blast_model: Option<String>,
    blast_sound: Option<String>,
}

#[derive(Resource, Default)]
struct Queued(Vec<Due>);

/// A projectile in the air. Its transform is the position in Bevy space.
#[derive(Component)]
pub struct Flight {
    model: String,
    /// Launched from a weapon's muzzle: drawn as a streak.
    bolt: bool,
    /// The muzzle flash model of the weapon that fired (a bolt).
    muzzle: Option<String>,
    from: Vec3,
    to: Vec3,
    /// Time it left, and seconds to `to`.
    start: f32,
    flight: f32,
    /// Peak height above the straight line (0 for a straight shot).
    arc: f32,
    spin: bool,
    end: ProjectileEnd,
    /// The shooter's position (where a deflected bolt bounces back toward).
    origin: Vec3,
    blast_model: Option<String>,
    blast_sound: Option<String>,
}

/// What a projectile did on reaching `at` (Bevy space). Miss is not recorded.
pub struct Landing {
    at: Vec3,
    end: ProjectileEnd,
    blast_model: Option<String>,
    blast_sound: Option<String>,
}

#[derive(Resource, Default)]
pub struct Landed(pub Vec<Landing>);

/// Assets the projectile visuals reuse: bolt looks, streak materials and the
/// flash sphere and its materials (one per colour).
#[derive(Resource, Default)]
pub(crate) struct FxCache {
    /// Bolt model -> (streak texture, light colour).
    looks: HashMap<String, (String, Color)>,
    streak_mats: HashMap<String, Handle<KMaterial>>,
    streak_mesh: Option<Handle<Mesh>>,
    flash_mesh: Option<Handle<Mesh>>,
    /// Flash material per colour (the colour's sRGB bits).
    flash_mats: HashMap<[u32; 3], Handle<StandardMaterial>>,
}

/// Removes the entity at `Expire` seconds (elapsed time).
#[derive(Component)]
struct Expire(f32);

/// A short flash that grows and is removed after `life` seconds.
#[derive(Component)]
struct Burst {
    start: f32,
    life: f32,
}

/// Clip to play on an animated root once its AnimPlayer exists.
#[derive(Component)]
struct PendingClip(&'static str);

/// Asset stores the projectile visuals need, bundled to keep system signatures short.
#[derive(SystemParam)]
pub(crate) struct Art<'w> {
    game: Res<'w, GameRes>,
    cache: ResMut<'w, AssetCache>,
    images: ResMut<'w, Assets<Image>>,
    meshes: ResMut<'w, Assets<Mesh>>,
    kmats: ResMut<'w, Assets<KMaterial>>,
    smats: ResMut<'w, Assets<StandardMaterial>>,
}

impl Art<'_> {
    pub(crate) fn loader(&mut self) -> Loader<'_> {
        Loader { res: &self.game.0, cache: &mut self.cache, images: &mut self.images, meshes: &mut self.meshes, materials: &mut self.kmats }
    }
}

/// A new area drops what is in the air or waiting to launch: the queued
/// launches, the landings not yet played, and the flights, flashes and
/// explosions (they are not under the area root, so they would stay).
fn reset_on_module(mut commands: Commands, events: Res<FrameEvents>, mut queued: ResMut<Queued>, mut landed: ResMut<Landed>, live: Query<Entity, Or<(With<Flight>, With<Burst>, With<Expire>)>>) {
    if !events.0.iter().any(|e| matches!(e, WorldEvent::ModuleEntered { .. })) {
        return;
    }
    queued.0.clear();
    landed.0.clear();
    for e in &live {
        commands.entity(e).despawn();
    }
}

/// Takes the world's new projectile events into the launch queue.
fn queue(time: Res<Time>, events: Res<FrameEvents>, mut queued: ResMut<Queued>) {
    let now = time.elapsed_secs();
    for ev in &events.0 {
        let WorldEvent::Projectile { shooter, model, hook, bolt, muzzle, to, launch, flight, arc, end, blast_model, blast_sound, .. } = ev else { continue };
        queued.0.push(Due {
            at: now + launch,
            shooter: *shooter,
            model: model.clone(),
            hand: hook.clone(),
            bolt: *bolt,
            muzzle: muzzle.clone(),
            to: to_bevy(*to),
            flight: *flight,
            arc: *arc,
            end: *end,
            blast_model: blast_model.clone(),
            blast_sound: blast_sound.clone(),
        });
    }
}

/// The launch point in the shooter's hand `hand` (`rhand` or `lhand`): for a
/// bolt, the `bullethook` of the weapon held there; otherwise the hand node.
/// None when the shooter has no such node (the caller uses a point above its feet).
fn hook_point(
    hand: &str,
    bolt: bool,
    root: Entity,
    roots: &Query<&ObjBody>,
    bindings: &Query<&AnimBindings>,
    kids: &Query<&Children>,
    names: &Query<&Name>,
    transforms: &Query<&GlobalTransform>,
) -> Option<Vec3> {
    let body = roots.get(root).ok()?.0?;
    let node = bindings.get(body).ok()?.entity(hand)?;
    if !bolt {
        return transforms.get(node).ok().map(|t| t.translation());
    }
    // The held weapon is a child of the hand node; its muzzle is the bullethook.
    let mut stack: Vec<Entity> = kids.get(node).map(|c| c.iter().collect()).unwrap_or_default();
    while let Some(e) = stack.pop() {
        if names.get(e).is_ok_and(|n| n.as_str().eq_ignore_ascii_case("bullethook")) {
            return transforms.get(e).ok().map(|t| t.translation());
        }
        if let Ok(c) = kids.get(e) {
            stack.extend(c.iter());
        }
    }
    None
}

/// Puts due projectiles in the air: a `Flight` at the launch point.
#[allow(clippy::too_many_arguments)]
fn launch(
    mut commands: Commands,
    time: Res<Time>,
    mut queued: ResMut<Queued>,
    ents: Res<ObjEntities>,
    roots: Query<&ObjBody>,
    bindings: Query<&AnimBindings>,
    kids: Query<&Children>,
    names: Query<&Name>,
    transforms: Query<&GlobalTransform>,
) {
    let now = time.elapsed_secs();
    let (due, wait): (Vec<Due>, Vec<Due>) = std::mem::take(&mut queued.0).into_iter().partition(|d| d.at <= now);
    queued.0 = wait;
    for d in due {
        // A shooter that is gone (or not yet placed) has nowhere to launch from.
        let Some(root) = ents.0.get(&d.shooter).copied() else { continue };
        let Ok(origin) = transforms.get(root).map(|t| t.translation()) else { continue };
        // No hook node: the chest-high point above the feet.
        let from = hook_point(&d.hand, d.bolt, root, &roots, &bindings, &kids, &names, &transforms).unwrap_or(origin + Vec3::Y * HOOK_HEIGHT);
        let dist = (d.to - from).length();
        let start = d.at;
        let arc = if d.arc { (0.25 * dist + 0.8).min(4.0) } else { 0.0 };
        commands.spawn((
            Name::new("projectile"),
            Transform::from_translation(from),
            Visibility::default(),
            Flight {
                model: d.model,
                bolt: d.bolt,
                muzzle: d.muzzle,
                from,
                to: d.to,
                start,
                flight: d.flight.max(0.05),
                arc,
                spin: d.arc,
                end: d.end,
                origin,
                blast_model: d.blast_model,
                blast_sound: d.blast_sound,
            },
        ));
    }
}

/// The light colour and streak texture of a bolt model: its first emitter's
/// texture and its light node's colour key.
fn bolt_look(game: &Resources, model: &str) -> Option<(String, Color)> {
    let bytes = game.get(&ResKey::new(model, ResourceType::Mdl)).ok().flatten()?;
    let mdx = game.get(&ResKey::new(model, ResourceType::Mdx)).ok().flatten();
    let mdl = k2_mdl::read_mdl_from_bytes(&bytes, mdx.as_deref()).ok()?;
    fn walk(n: &MdlNode, texture: &mut Option<String>, color: &mut Option<[f32; 3]>) {
        match &n.node_data {
            MdlNodeData::Emitter(e) if texture.is_none() && !e.texture.trim().is_empty() => *texture = Some(e.texture.trim().to_ascii_lowercase()),
            MdlNodeData::Light(_) if color.is_none() => {
                let key = n.controllers.iter().find(|c| c.controller_type == MdlControllerType::COLOR).and_then(|c| c.keys.first());
                if let Some(v) = key.map(|k| &k.values).filter(|v| v.len() >= 3) {
                    *color = Some([v[0], v[1], v[2]]);
                }
            }
            _ => {}
        }
        for c in &n.children {
            walk(c, texture, color);
        }
    }
    let (mut texture, mut color) = (None, None);
    walk(&mdl.root_node, &mut texture, &mut color);
    let tex = texture.unwrap_or_else(|| "fx_laser_01".into());
    let [r, g, b] = color.unwrap_or([1.0, 0.2, 0.1]);
    Some((tex, Color::srgb(r.clamp(0.0, 1.0), g.clamp(0.0, 1.0), b.clamp(0.0, 1.0))))
}

/// Gives each new flight its look: bolt streaks and light with the muzzle
/// flash, or the item model.
fn dress(mut commands: Commands, mut art: Art, mut fx: ResMut<FxCache>, fresh: Query<(Entity, &Flight), Added<Flight>>) {
    for (e, f) in &fresh {
        if !f.bolt {
            if let Some(t) = art.loader().model(&f.model) {
                spawn_tree(&mut commands, &t, e, Transform::IDENTITY, None);
            }
            continue;
        }
        let (tex, color) = match fx.looks.get(&f.model) {
            Some(l) => l.clone(),
            None => {
                let look = bolt_look(&art.game.0, &f.model).unwrap_or_else(|| ("fx_laser_01".into(), Color::srgb(1.0, 0.2, 0.1)));
                fx.looks.insert(f.model.clone(), look.clone());
                look
            }
        };
        let mat = match fx.streak_mats.get(&tex) {
            Some(h) => h.clone(),
            None => {
                let h = art.loader().texture_material(&tex);
                let mut m = art.kmats.get(&h).cloned().expect("texture_material adds its material");
                m.blend = Blend::Additive;
                m.params.flags |= FLAG_ADDITIVE;
                let own = art.kmats.add(m);
                fx.streak_mats.insert(tex.clone(), own.clone());
                own
            }
        };
        let mesh = fx.streak_mesh.get_or_insert_with(|| art.meshes.add(Rectangle::new(STREAK_WIDTH, STREAK_LENGTH))).clone();
        let flash_spec = f.muzzle.clone().map(|body| CreatureSpec { body, head: None, texture: None, own_idle: true });
        let muzzle = flash_spec.and_then(|spec| spawn_creature(&mut commands, &mut art.loader(), &spec, e, Transform::IDENTITY));
        if let Some(m) = muzzle {
            commands.entity(m).insert((FxLive, PendingClip("impact")));
        }
        commands.entity(e).with_children(|c| {
            // Two crossed streaks, so the bolt reads from any side.
            for turn in [0.0, FRAC_PI_2] {
                c.spawn((Name::new("bolt streak"), Mesh3d(mesh.clone()), MeshMaterial3d(mat.clone()), Transform::from_rotation(Quat::from_rotation_y(turn)), bevy::light::NotShadowCaster));
            }
            c.spawn((Name::new("bolt light"), PointLight { color, intensity: BOLT_LIGHT, range: BOLT_LIGHT_RANGE, shadow_maps_enabled: false, ..default() }));
        });
    }
}

/// Moves every flight along its path. At the end: a deflected bolt bounces
/// back (and is a miss from then on); anything else is recorded and removed.
fn fly(mut commands: Commands, time: Res<Time>, mut landed: ResMut<Landed>, mut flights: Query<(Entity, &mut Flight, &mut Transform)>) {
    let now = time.elapsed_secs();
    for (e, mut f, mut tf) in &mut flights {
        let s = ((now - f.start) / f.flight).clamp(0.0, 1.0);
        tf.translation = f.from.lerp(f.to, s) + Vec3::Y * (4.0 * f.arc * s * (1.0 - s));
        let dir = (f.to - f.from).normalize_or(Vec3::Y);
        tf.rotation = if f.spin { Quat::from_euler(EulerRot::XYZ, now * 5.0, now * 3.0, 0.0) } else { Quat::from_rotation_arc(Vec3::Y, dir) };
        if s < 1.0 {
            continue;
        }
        match f.end {
            ProjectileEnd::Miss => commands.entity(e).despawn(),
            ProjectileEnd::Deflect => {
                landed.0.push(Landing { at: f.to, end: ProjectileEnd::Deflect, blast_model: None, blast_sound: None });
                // Back toward the shooter's hook. The bounce is the lesser of 12 m and the
                // distance back; its direction wobbles a little, so it ends near the shooter.
                let hook = f.origin + Vec3::Y * HOOK_HEIGHT;
                let back = (hook - f.to).normalize_or(Vec3::Z);
                let side = back.cross(Vec3::Y).normalize_or_zero();
                let dir = (back + side * (now * 37.0).sin() * 0.6).normalize_or(back);
                let length = BOUNCE_LENGTH.min((hook - f.to).length());
                f.from = f.to;
                f.to += dir * length;
                f.start += f.flight;
                f.flight = BOUNCE_TIME;
                f.arc = 0.0;
                f.end = ProjectileEnd::Miss;
            }
            end => {
                landed.0.push(Landing { at: f.to, end, blast_model: f.blast_model.clone(), blast_sound: f.blast_sound.clone() });
                commands.entity(e).despawn();
            }
        }
    }
}

/// The flash sphere, shared by every flash.
fn flash_mesh(fx: &mut FxCache, meshes: &mut Assets<Mesh>) -> Handle<Mesh> {
    fx.flash_mesh.get_or_insert_with(|| meshes.add(Sphere::new(FLASH_RADIUS))).clone()
}

/// The additive material of a flash colour, shared by flashes of that colour.
fn flash_material(fx: &mut FxCache, smats: &mut Assets<StandardMaterial>, color: Color) -> Handle<StandardMaterial> {
    let c = color.to_srgba();
    let key = [c.red, c.green, c.blue].map(f32::to_bits);
    fx.flash_mats
        .entry(key)
        .or_insert_with(|| smats.add(StandardMaterial { base_color: color.with_alpha(0.5), emissive: color.to_linear() * 6.0, alpha_mode: AlphaMode::Add, unlit: true, ..default() }))
        .clone()
}

/// A flash at `at`: an additive sphere with a light, growing and gone in FLASH_LIFE.
pub(crate) fn burst(commands: &mut Commands, art: &mut Art, fx: &mut FxCache, at: Vec3, now: f32, color: Color, intensity: f32) {
    let mesh = flash_mesh(fx, &mut art.meshes);
    let mat = flash_material(fx, &mut art.smats, color);
    commands.spawn((
        Name::new("projectile flash"),
        Transform::from_translation(at),
        Visibility::default(),
        Mesh3d(mesh),
        MeshMaterial3d(mat),
        PointLight { color, intensity, range: 5.0, shadow_maps_enabled: false, ..default() },
        Burst { start: now, life: FLASH_LIFE },
    ));
}

/// Plays what the landed projectiles leave: flashes, the explosion model with
/// its clip and sound.
fn land(mut commands: Commands, time: Res<Time>, mut landed: ResMut<Landed>, mut art: Art, mut fx: ResMut<FxCache>, mut voice: Voice) {
    let now = time.elapsed_secs();
    for l in std::mem::take(&mut landed.0) {
        match l.end {
            ProjectileEnd::Impact => burst(&mut commands, &mut art, &mut fx, l.at, now, Color::srgb(1.0, 0.8, 0.5), 20_000.0),
            ProjectileEnd::Deflect => burst(&mut commands, &mut art, &mut fx, l.at, now, Color::srgb(0.6, 0.8, 1.0), 20_000.0),
            ProjectileEnd::Detonate => {
                let holder = commands.spawn((Name::new("blast"), Transform::from_translation(l.at), Visibility::default(), Expire(now + BLAST_LIFE))).id();
                if let Some(model) = &l.blast_model {
                    let spec = CreatureSpec { body: model.clone(), head: None, texture: None, own_idle: true };
                    if let Some(b) = spawn_creature(&mut commands, &mut art.loader(), &spec, holder, Transform::IDENTITY) {
                        commands.entity(b).insert((FxLive, PendingClip("impact")));
                    }
                }
                if let Some(sound) = &l.blast_sound {
                    voice.play_sound_at(sound, holder, Vec3::ZERO, BLAST_RANGE);
                }
                burst(&mut commands, &mut art, &mut fx, l.at, now, Color::srgb(1.0, 0.6, 0.2), 60_000.0);
            }
            ProjectileEnd::Miss => {}
        }
    }
}

/// Starts the clip an explosion or muzzle flash waits for, once its AnimPlayer exists.
fn start_clips(mut commands: Commands, mut q: Query<(Entity, &PendingClip, &mut AnimPlayer)>) {
    for (e, clip, mut p) in &mut q {
        p.play(clip.0, false, 1.0);
        commands.entity(e).remove::<PendingClip>();
    }
}

fn grow_bursts(mut commands: Commands, time: Res<Time>, mut q: Query<(Entity, &Burst, &mut Transform)>) {
    let now = time.elapsed_secs();
    for (e, b, mut tf) in &mut q {
        let t = (now - b.start) / b.life;
        if t >= 1.0 {
            commands.entity(e).despawn();
            continue;
        }
        tf.scale = Vec3::splat(0.5 + 1.5 * t);
    }
}

fn expire(mut commands: Commands, time: Res<Time>, q: Query<(Entity, &Expire)>) {
    let now = time.elapsed_secs();
    for (e, x) in &q {
        if now >= x.0 {
            commands.entity(e).despawn();
        }
    }
}

#[cfg(test)]
mod tests;

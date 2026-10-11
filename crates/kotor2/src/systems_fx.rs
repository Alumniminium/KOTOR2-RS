//! Mines, traps and stealth on screen (k2-world traps.rs / stealth.rs).
//!
//! - Markers: every armed trap the party knows about shows its traps.2da
//!   model (v_mnfrag, v_mnstun, ...) on a glowing disc: red for hostile
//!   traps, green for the party's own mines.
//! - Explosions: TrapTriggered flashes a light and plays the traps.2da
//!   explosionsound at the trap.
//! - Stealth: a stealthed creature is drawn with a dithered, shimmering
//!   see-through material (FLAG_STEALTH); leaving stealth restores it.
//!   Stealth disables running for the leader, as in KotOR.

use std::collections::HashMap;

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::WorldEvent;

use crate::audio::{AudioCache, SoundIndex, spawn_positional};
use crate::game::{AreaRoot, FrameEvents, GameSet, ObjEntities, WorldRes};
use crate::material::{Blend, FLAG_ALPHA_MASK, FLAG_STEALTH, KMaterial, KParams};
use crate::model::{AssetCache, GameRes, Loader, spawn_template};
use crate::play::PlayerMoveState;

pub struct SystemsFxPlugin;

impl Plugin for SystemsFxPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Markers>()
            .init_resource::<StealthMats>()
            .add_systems(Update, (test_hook, sync_markers, explosions, stealth_looks, stealth_no_run, pulse).chain().in_set(GameSet::Ui))
            .add_systems(PostUpdate, apply_stealth);
    }
}

/// Trap object -> marker entity.
#[derive(Resource, Default)]
struct Markers(HashMap<ObjectId, Entity>);

#[derive(Component)]
struct TrapMarker;

#[derive(Component)]
struct MarkerDisc;

#[derive(Component)]
struct Flash {
    left: f32,
    total: f32,
}

#[allow(clippy::too_many_arguments)]
fn sync_markers(
    mut commands: Commands,
    world: Res<WorldRes>,
    game: Res<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut std_mats: ResMut<Assets<StandardMaterial>>,
    mut markers: ResMut<Markers>,
    area: Query<Entity, With<AreaRoot>>,
    alive: Query<(), With<TrapMarker>>,
) {
    let Ok(area) = area.single() else {
        markers.0.clear();
        return;
    };
    let w = &world.0;
    let visible = w.visible_traps();
    // Drop markers of traps that are gone (or whose entity went with the area).
    markers.0.retain(|id, e| {
        let keep = alive.get(*e).is_ok() && visible.iter().any(|m| m.id == *id);
        if !keep && alive.get(*e).is_ok() {
            commands.entity(*e).despawn();
        }
        keep
    });
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    for m in visible {
        if markers.0.contains_key(&m.id) {
            continue;
        }
        let root = commands
            .spawn((Name::new(format!("trap marker {}", m.id)), TrapMarker, Transform::from_translation(m.position + Vec3::Z * 0.02), Visibility::default(), ChildOf(area)))
            .id();
        if let Some(t) = m.model.as_deref().and_then(|name| loader.model(name)) {
            spawn_template(&mut commands, &t, root, Transform::IDENTITY);
        }
        let color = if m.hostile { LinearRgba::rgb(1.0, 0.15, 0.1) } else { LinearRgba::rgb(0.2, 1.0, 0.3) };
        let disc = loader.meshes.add(Cylinder::new(0.55, 0.01));
        let mat = std_mats.add(StandardMaterial {
            base_color: Color::LinearRgba(color.with_alpha(0.35)),
            emissive: color * 2.0,
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        });
        // Cylinder is Y-up; the area root is Z-up game space.
        commands.spawn((
            MarkerDisc,
            Mesh3d(disc),
            MeshMaterial3d(mat),
            Transform::from_rotation(Quat::from_rotation_x(std::f32::consts::FRAC_PI_2)),
            ChildOf(root),
        ));
        markers.0.insert(m.id, root);
        info!("trap marker: {} at {:?} hostile={} model={:?}", m.id, m.position, m.hostile, m.model);
    }
}

/// Discs breathe a little so they read as live.
fn pulse(time: Res<Time>, mut discs: Query<&mut Transform, With<MarkerDisc>>) {
    let s = 1.0 + 0.08 * (time.elapsed_secs() * 3.0).sin();
    for mut t in &mut discs {
        t.scale = Vec3::new(s, 1.0, s);
    }
}

#[allow(clippy::too_many_arguments)]
fn explosions(
    mut commands: Commands,
    time: Res<Time>,
    events: Res<FrameEvents>,
    game: Res<GameRes>,
    index: Res<SoundIndex>,
    mut cache: ResMut<AudioCache>,
    mut assets: ResMut<Assets<bevy::audio::AudioSource>>,
    area: Query<Entity, With<AreaRoot>>,
    mut flashes: Query<(Entity, &mut Flash, &mut PointLight)>,
) {
    for (e, mut f, mut light) in &mut flashes {
        f.left -= time.delta_secs();
        if f.left <= 0.0 {
            commands.entity(e).despawn();
        } else {
            light.intensity = 4.0e6 * (f.left / f.total).powi(2);
        }
    }
    let Ok(area) = area.single() else { return };
    for e in &events.0 {
        let WorldEvent::TrapTriggered { position, sound, trap_type, .. } = e else { continue };
        info!("trap: type {trap_type} goes off at {position:?} sound {sound:?}");
        let at = commands
            .spawn((
                Name::new("trap blast"),
                Flash { left: 0.6, total: 0.6 },
                PointLight { color: Color::srgb(1.0, 0.7, 0.35), intensity: 4.0e6, range: 12.0, shadow_maps_enabled: false, ..default() },
                Transform::from_translation(*position + Vec3::Z * 0.6),
                Visibility::default(),
                ChildOf(area),
            ))
            .id();
        if let Some(s) = sound {
            let played = spawn_positional(&mut commands, &mut cache, &game.0, &index, &mut assets, at, s, (2.0, 30.0, 1.0));
            if played.is_none() {
                warn!("trap: explosion sound {s} not found");
            }
        }
    }
}

/// Marks object roots of stealthed creatures.
#[derive(Component)]
struct StealthLook;

/// Mesh switched to the stealth material; holds the original.
#[derive(Component)]
struct StealthOrig(Handle<KMaterial>);

#[derive(Resource, Default)]
struct StealthMats(HashMap<AssetId<KMaterial>, Handle<KMaterial>>);

fn stealth_looks(mut commands: Commands, world: Res<WorldRes>, ents: Res<ObjEntities>, looks: Query<Entity, With<StealthLook>>) {
    let w = &world.0;
    let mut want = Vec::new();
    for (&id, &e) in &ents.0 {
        if w.is_stealthed(id) {
            want.push(e);
        }
    }
    for e in &looks {
        if !want.contains(&e) {
            commands.entity(e).remove::<StealthLook>();
        }
    }
    for e in want {
        if !looks.contains(e) {
            commands.entity(e).insert(StealthLook);
        }
    }
}

fn apply_stealth(
    mut commands: Commands,
    time: Res<Time>,
    roots: Query<(Entity, Has<StealthLook>), With<crate::game::ObjId>>,
    children: Query<&Children>,
    mut meshes: Query<(&mut MeshMaterial3d<KMaterial>, Option<&StealthOrig>)>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut cache: ResMut<StealthMats>,
) {
    for (root, stealthed) in &roots {
        for e in children.iter_descendants(root) {
            let Ok((mut mat, orig)) = meshes.get_mut(e) else { continue };
            match (stealthed, orig) {
                (true, None) => {
                    let id = mat.0.id();
                    let h = match cache.0.get(&id) {
                        Some(h) => h.clone(),
                        None => {
                            let Some(src) = materials.get(id).cloned() else { continue };
                            let mut m = src;
                            m.params = KParams { flags: m.params.flags | FLAG_STEALTH | FLAG_ALPHA_MASK, alpha_cut: m.params.alpha_cut.max(0.5), ..m.params };
                            m.blend = Blend::Opaque;
                            let h = materials.add(m);
                            cache.0.insert(id, h.clone());
                            h
                        }
                    };
                    commands.entity(e).insert(StealthOrig(mat.0.clone()));
                    mat.0 = h;
                }
                (false, Some(o)) => {
                    mat.0 = o.0.clone();
                    commands.entity(e).remove::<StealthOrig>();
                }
                _ => {}
            }
        }
    }
    // Shimmer phase, shared by every stealth material.
    let t = time.elapsed_secs();
    for h in cache.0.values() {
        if let Some(mut m) = materials.get_mut(h) {
            m.params._pad1 = t;
        }
    }
}

/// KotOR: "Stealth Mode Activated (Run Disabled)".
fn stealth_no_run(world: Res<WorldRes>, ents: Res<ObjEntities>, mut players: Query<(Entity, &mut PlayerMoveState)>) {
    let leader = world.0.party_leader();
    if !world.0.is_stealthed(leader) {
        return;
    }
    let Some(&e) = ents.0.get(&leader) else { return };
    if let Ok((_, mut st)) = players.get_mut(e)
        && st.running
    {
        st.running = false;
    }
}

/// `K2_SYS_TEST=markers`: the leader gets Awareness 60, stands 3 m from the
/// area's first trap and plants a frag mine of its own (red + green
/// markers). `K2_SYS_TEST=stealth`: the leader equips a Stealth Field
/// Generator and enters stealth. Runs once, 2 s into the area.
fn test_hook(
    time: Res<Time>,
    mut world: ResMut<WorldRes>,
    ents: Res<ObjEntities>,
    mut players: Query<&mut crate::play::Player>,
    mut done: Local<Option<f32>>,
) {
    let Ok(mode) = std::env::var("K2_SYS_TEST") else { return };
    let w = &mut world.0;
    let leader = w.party_leader();
    if !ents.0.contains_key(&leader) {
        return;
    }
    let t = done.get_or_insert(time.elapsed_secs() + 2.0);
    if *t < 0.0 || time.elapsed_secs() < *t {
        return;
    }
    *t = -1.0;
    match mode.as_str() {
        "markers" => {
            w.set_skill_rank(leader, k2_world::traps::SKILL_AWARENESS, 60);
            let trap = w.iter().find(|o| o.trap.as_ref().is_some_and(|t| !t.disabled)).map(|o| o.id);
            if let Some(trap) = trap
                && let Some(p) = w.trap_position(trap)
            {
                let mut spot = p + Vec3::new(3.0, 0.0, 0.0);
                for k in 0..12 {
                    let a = k as f32 * std::f32::consts::TAU / 12.0;
                    let q = p + Vec3::new(a.cos(), a.sin(), 0.0) * 3.0;
                    if let Some(g) = w.walk.ground(q.x, q.y, p.z + 1.0) {
                        spot = Vec3::new(q.x, q.y, g.z);
                        break;
                    }
                }
                w.place(leader, spot);
                let d = (p - spot).truncate();
                // Face away from the trap so the own mine lands beside it.
                let bearing = -(-d.x).atan2(-d.y);
                if let Some(o) = w.get_mut(leader) {
                    o.bearing = bearing;
                }
                // The controlled character's position lives in Player too.
                for mut pl in &mut players {
                    pl.pos = spot;
                    pl.facing = bearing + std::f32::consts::FRAC_PI_2;
                }
                info!("systest: leader at {spot:?}, trap {trap} at {p:?}");
            }
            if let Some(item) = w.give_new_item("g_i_trapkit004", leader, 1, true) {
                match w.set_mine(leader, item) {
                    Ok(m) => info!("systest: own mine {m}"),
                    Err(e) => warn!("systest: set mine: {e}"),
                }
            }
        }
        "stealth" => {
            if let Some(belt) = w.create_item("a_belt_03", leader, 1) {
                let _ = w.equip(leader, belt, 10);
            }
            w.set_skill_rank(leader, 2, 30);
            match w.set_stealth(leader, true) {
                Ok(()) => info!("systest: stealth on"),
                Err(e) => warn!("systest: stealth: {e}"),
            }
        }
        _ => {}
    }
}

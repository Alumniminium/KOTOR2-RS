//! The Bevy side of k2-world: owns the `World`, ticks it, loads areas and
//! mirrors world objects as entities.
//!
//! Frame order (`GameSet`): Load (area reload) -> Input (player movement,
//! interaction) -> Tick (world.tick, drain events, transitions) -> Sync
//! (spawn/despawn/move entities, animations) -> Ui.
//!
//! World objects of kind creature, door and placeable get a root entity
//! under the `AreaRoot` (game space) with `ObjId`; their model is a child
//! (`ObjBody`). Reloading an area despawns the area root and everything
//! under it.

use std::collections::HashMap;
use std::f32::consts::FRAC_PI_2;
use std::sync::Arc;

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::{Motion, Object, ObjectKind, World, WorldEvent};

use crate::Args;
use crate::anim::{AnimPlayer, AnimSet, CreatureSpec, IDLE_ANIMS, spawn_creature};
use crate::area::{AreaModule, Interactable};
use crate::coords::WorldRoot;
use crate::hover::Target;
use crate::material::KMaterial;
use crate::messages::AreaLoaded;
use crate::model::{self, AssetCache, GameRes, Loader};
use crate::play::{self, ClickWalk, Player, PlayerMoveState};

/// The authoritative simulation.
#[derive(Resource)]
pub struct WorldRes(pub World);

#[derive(States, Default, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum AppState {
    #[default]
    Menu,
    Game,
}

#[derive(SystemSet, Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum GameSet {
    Load,
    Input,
    Tick,
    Sync,
    Ui,
}

/// Set when the world entered a module and the Bevy side must rebuild.
#[derive(Resource, Default)]
pub struct NeedAreaLoad(pub bool);

/// Parent of the current area's rooms and objects (child of `WorldRoot`).
#[derive(Component)]
pub struct AreaRoot;

/// The world object an entity mirrors.
#[derive(Component, Clone, Copy, Debug)]
pub struct ObjId(pub ObjectId);

/// The model root (with `AnimPlayer`) under an object root, if it has one.
#[derive(Component, Clone, Copy)]
pub struct ObjBody(pub Option<Entity>);

/// Creature walk/run cycle distances (appearance walkdist/rundist) and the
/// mirrored ground speed.
#[derive(Component, Default)]
pub struct CreatureAnim {
    pub walk_dist: f32,
    pub run_dist: f32,
    pub speed: f32,
    prev: Option<Vec3>,
    /// Scripted animation overriding idle: clip, looping, end time. A
    /// one-shot with an infinite end time holds its last frame (cutscene
    /// poses such as the 101per collapse).
    pub(crate) scripted: Option<(String, bool, Option<f32>)>,
}

/// After a one-shot on a door or placeable ends, hold this clip.
#[derive(Component)]
struct HoldAfter(String);

#[derive(Resource, Default)]
pub struct ObjEntities(pub HashMap<ObjectId, Entity>);

/// Esc pause menu is open (world and animation time stopped).
#[derive(Resource, Default)]
pub struct Paused(pub bool);

/// A conversation is running (dialog UI and camera own the screen).
#[derive(Resource, Default)]
pub struct InConversation(pub bool);

/// World events drained this frame, for the UI systems.
#[derive(Resource, Default)]
pub struct FrameEvents(pub Vec<WorldEvent>);

/// Fade overlay state. `alpha` is the current opacity, moving to `target`
/// at `rate` per second after `wait` seconds.
#[derive(Resource)]
pub struct Fade {
    pub alpha: f32,
    pub target: f32,
    pub rate: f32,
    pub wait: f32,
    pub color: [f32; 3],
}

impl Default for Fade {
    fn default() -> Self {
        Self { alpha: 1.0, target: 0.0, rate: 2.0, wait: 0.0, color: [0.0; 3] }
    }
}

impl Fade {
    pub fn to(&mut self, target: f32, wait: f32, length: f32, color: [f32; 3]) {
        self.target = target;
        self.wait = wait.max(0.0);
        self.rate = if length > 0.01 { 1.0 / length } else { 1000.0 };
        self.color = color;
    }
}

/// Queued Bink movies. The world does not tick while one is up. With
/// `real` set, `movie.rs` plays and pops them; otherwise each is a black
/// card with the name for a second.
#[derive(Resource, Default)]
pub struct MovieCards {
    pub queue: Vec<String>,
    pub shown: f32,
    pub real: bool,
}

impl MovieCards {
    pub fn current(&self) -> Option<&str> {
        self.queue.first().map(String::as_str)
    }
}

/// A module transition in progress: fade out, then switch.
#[derive(Resource, Default)]
pub struct Transition(pub Option<(k2_world::PendingTransition, f32)>);

/// Floating lines the presentation adds itself (e.g. "Locked"), shown by
/// the HUD like script barks.
#[derive(Resource, Default)]
pub struct PendingBarks(pub Vec<(ObjectId, String)>);

/// The player clicked an object out of reach: walk there, then use it.
#[derive(Resource, Default)]
pub struct PendingUse(pub Option<ObjectId>);

/// What to do on arrival at a `PendingUse` target.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum PendingVerb {
    #[default]
    Use,
    /// Security skill (or key) on a locked door or container.
    Unlock,
}

/// Test hooks driven from the command line.
#[derive(Resource, Default)]
struct AutoState {
    since_load: f32,
    talked: bool,
    transitioned: bool,
    used: bool,
    attacked: bool,
    door_logged: bool,
}

/// The system that turns world events into animations; later consumers of
/// `FrameEvents` (sound effects) order themselves after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct ApplyEvents;

pub struct GamePlugin;

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<AppState>()
            .add_message::<AreaLoaded>()
            .init_resource::<NeedAreaLoad>()
            .init_resource::<ObjEntities>()
            .init_resource::<Paused>()
            .init_resource::<InConversation>()
            .init_resource::<FrameEvents>()
            .init_resource::<Fade>()
            .init_resource::<MovieCards>()
            .init_resource::<Transition>()
            .init_resource::<PendingUse>()
            .init_resource::<ClickWalk>()
            .init_resource::<PendingVerb>()
            .init_resource::<PendingBarks>()
            .init_resource::<AutoState>()
            .configure_sets(
                Update,
                (GameSet::Load, GameSet::Input, GameSet::Tick, GameSet::Sync, GameSet::Ui).chain().run_if(in_state(AppState::Game)),
            )
            .add_systems(Update, load_area.in_set(GameSet::Load))
            .add_systems(Update, (interact, auto_tests).chain().in_set(GameSet::Input).after(play::move_player))
            .add_systems(Update, (world_tick, transitions).chain().in_set(GameSet::Tick))
            .add_systems(Update, (sync_objects, apply_events.in_set(ApplyEvents), animate_creatures, settle_holds, corpse_targets).chain().in_set(GameSet::Sync))
            .add_systems(Update, death_shot);
    }
}

/// Builds the world for the install. The appearance override is applied to
/// the default player spec.
pub fn make_world(install: k2_res::Install, args: &Args) -> World {
    let mut w = World::new(install).unwrap_or_else(|e| {
        eprintln!("error: world: {e}");
        std::process::exit(1);
    });
    if let Some(a) = args.player_appearance {
        w.player_spec.appearance = a;
    }
    w
}

/// Enters `module` and schedules the Bevy-side rebuild.
pub fn enter_module(world: &mut World, need: &mut NeedAreaLoad, module: &str, waypoint: Option<&str>) -> bool {
    match world.enter_module(module, waypoint) {
        Ok(()) => {
            need.0 = true;
            true
        }
        Err(e) => {
            error!("enter module {module}: {e}");
            false
        }
    }
}

/// Rebuilds the area after `World::enter_module`: rooms, then objects are
/// spawned by `sync_objects` on the same frame.
#[allow(clippy::too_many_arguments)]
fn load_area(
    mut commands: Commands,
    mut need: ResMut<NeedAreaLoad>,
    mut world: ResMut<WorldRes>,
    mut game: ResMut<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut ents: ResMut<ObjEntities>,
    mut auto: ResMut<AutoState>,
    mut loaded: MessageWriter<AreaLoaded>,
    mut cams: Query<&mut play::PlayCam>,
    mut clickwalk: ResMut<ClickWalk>,
    mut pending: ResMut<PendingUse>,
    roots: Query<Entity, With<AreaRoot>>,
    world_root: Query<Entity, With<WorldRoot>>,
) {
    if !need.0 {
        return;
    }
    need.0 = false;
    // A click walk and its pending use belong to the area they were made in: their
    // object ids and search grids are the old area's.
    *clickwalk = ClickWalk::default();
    pending.0 = None;
    let t0 = std::time::Instant::now();
    for e in &roots {
        commands.entity(e).despawn();
    }
    ents.0.clear();
    // Test hooks fire once per run; only the clock restarts per area.
    auto.since_load = 0.0;
    let w = &mut world.0;
    let Some(name) = w.module.as_ref().map(|m| m.name.clone()) else { return };

    let res = &mut game.0;
    let module = match k2_data::Module::load(res, &name) {
        Ok(m) => Arc::new(m),
        Err(e) => {
            error!("load module {name}: {e}");
            return;
        }
    };
    let Ok(wr) = world_root.single() else { return };
    let area = commands.spawn((Name::new(format!("area {name}")), AreaRoot, Transform::default(), Visibility::default(), ChildOf(wr))).id();
    let mut loader = Loader { res, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };
    let mut rooms = 0;
    for room in &module.layout.rooms {
        if let Some(t) = loader.model(&room.model) {
            let r = model::spawn_template(&mut commands, &t, area, Transform::from_translation(Vec3::from(room.position)));
            commands.entity(r).insert(play::RoomModel);
            rooms += 1;
        }
    }
    commands.insert_resource(AreaModule(module.clone()));

    // Some entry points sit on walkmesh islands (101PER: under the bed);
    // move the party leader to open ground like the engine's safe location.
    let leader = w.party_leader();
    if let Some(p) = w.get(leader).map(|o| o.position)
        && let Some(safe) = w.walk.safe_spawn(p, w.leader_clearance())
        && safe.distance(p) > 0.01
    {
        info!("leader moved from {p} to safe spot {safe}");
        if let Some(o) = w.get_mut(leader) {
            o.position = safe;
        }
    }
    if let Some(b) = w.get(leader).map(|o| o.bearing) {
        play::reset_camera_behind(&mut cams, b + FRAC_PI_2);
    }
    info!("area {name}: {rooms} rooms in {:?}", t0.elapsed());
    loaded.write(AreaLoaded { module: name });
}

/// Ticks the world unless paused, a movie card is up, or a transition is
/// fading out; collects this frame's events.
pub(crate) fn world_tick(
    time: Res<Time<Real>>,
    paused: Res<Paused>,
    mut cards: ResMut<MovieCards>,
    transition: Res<Transition>,
    mut world: ResMut<WorldRes>,
    mut events: ResMut<FrameEvents>,
    mut conv: ResMut<InConversation>,
    mut gate: ResMut<crate::presentation::Gate>,
    vclock: Res<Time<bevy::time::Virtual>>,
) {
    events.0.clear();
    let dt = time.delta_secs().min(0.1);
    if paused.0 {
        // No time passes, but what menus change (equipment, armour, items)
        // must reach the models while they are open. The gate runs on the
        // virtual clock, as the bolts do.
        events.0 = gate.filter(vclock.elapsed_secs(), world.0.drain_events());
        note_events(&world.0, &events.0, &mut cards);
        return;
    }
    if cards.current().is_some() {
        if cards.real {
            return;
        }
        cards.shown += dt;
        if cards.shown >= 1.0 {
            cards.queue.remove(0);
            cards.shown = 0.0;
        }
        return;
    }
    if transition.0.is_some() {
        return;
    }
    let w = &mut world.0;
    w.tick(dt);
    events.0 = gate.filter(vclock.elapsed_secs(), w.drain_events());
    note_events(w, &events.0, &mut cards);
    conv.0 = w.conversation.is_some();
    for msg in w.vm_errors.drain(..) {
        warn!("script: {msg}");
    }
}

/// Logs the events `world_tick` acts on itself and queues movie cards.
fn note_events(w: &World, events: &[WorldEvent], cards: &mut MovieCards) {
    for e in events {
        match e {
            WorldEvent::ConversationStarted { owner, dialog } => info!("conversation {dialog} owner {}", w.describe(*owner)),
            WorldEvent::ConversationEnded { owner, aborted } => info!("conversation ended ({}) aborted={aborted}", w.describe(*owner)),
            WorldEvent::TransitionRequested(t) => info!("transition requested to {} ({:?})", t.module, t.waypoint_tag),
            WorldEvent::Movie(m) => {
                info!("movie {m} queued");
                cards.queue.push(m.clone());
            }
            _ => {}
        }
    }
}

/// Fades out, enters the pending module, fades back in.
#[allow(clippy::too_many_arguments)]
fn transitions(
    time: Res<Time<Real>>,
    mut world: ResMut<WorldRes>,
    mut transition: ResMut<Transition>,
    mut fade: ResMut<Fade>,
    mut need: ResMut<NeedAreaLoad>,
    mut cards: ResMut<MovieCards>,
    mut events: ResMut<FrameEvents>,
) {
    let w = &mut world.0;
    if transition.0.is_none()
        && let Some(t) = w.pending_transition.take()
    {
        fade.to(1.0, 0.0, 0.35, [0.0; 3]);
        transition.0 = Some((t, 0.0));
    }
    let Some((t, elapsed)) = &mut transition.0 else { return };
    *elapsed += time.delta_secs();
    if *elapsed < 0.4 {
        return;
    }
    let t = t.clone();
    transition.0 = None;
    info!("entering {} at {:?}", t.module, t.waypoint_tag);
    cards.queue.extend(t.movies.iter().cloned());
    if enter_module(w, &mut need, &t.module, t.waypoint_tag.as_deref()) {
        fade.alpha = 1.0;
        fade.to(0.0, 0.2, 0.5, [0.0; 3]);
        // Load-time events (fades from OnModLoad, ...) go to this frame.
        events.0.extend(w.drain_events());
    }
}

/// Model name and kind-specific info for an object that gets an entity.
fn object_model(o: &Object) -> Option<CreatureSpec> {
    match o.kind {
        ObjectKind::Creature => {
            let c = o.creature()?;
            let body = c.body_model.clone()?;
            // Stunt models (p_kreiastunt) hold their own lying pose, authored
            // in area space, instead of the humanoid idle.
            let own_idle = is_stunt_model(&body);
            Some(CreatureSpec { body, head: c.head_model.clone(), texture: c.body_texture.clone(), own_idle })
        }
        ObjectKind::Door => Some(CreatureSpec { body: o.door()?.model.clone()?, head: None, texture: None, own_idle: false }),
        ObjectKind::Placeable => Some(CreatureSpec { body: o.placeable()?.model.clone()?, head: None, texture: None, own_idle: true }),
        _ => None,
    }
}

fn is_stunt_model(name: &str) -> bool {
    name.to_ascii_lowercase().contains("stunt")
}

fn interactable(w: &World, o: &Object, leader: ObjectId) -> Option<Interactable> {
    let name = w.substitute(&o.name);
    match o.kind {
        ObjectKind::Creature if o.id != leader => {
            let c = o.creature()?;
            Some(Interactable { name, radius: c.radius.max(0.4), height: 2.0, useable: true })
        }
        // A static door does not open, but one with a conversation talks on click
        // (the cantina doors: "magnetically sealed").
        ObjectKind::Door if !o.door()?.is_static || o.conversation.as_deref().is_some_and(|c| !c.is_empty()) => Some(Interactable {
            name: if name.is_empty() { "Door".into() } else { name },
            radius: 1.2,
            height: 2.2,
            useable: true,
        }),
        ObjectKind::Placeable if o.placeable()?.useable => Some(Interactable { name, radius: 0.8, height: 1.2, useable: true }),
        _ => None,
    }
}

/// Uses `target`, with a "Locked" line over locked doors and containers.
/// Security (or a key) on a locked object, with a floating result line.
pub fn unlock_with_feedback(w: &mut World, barks: &mut PendingBarks, leader: ObjectId, target: ObjectId) {
    info!("unlock {}", w.describe(target));
    match w.try_unlock(leader, target) {
        Ok(()) => barks.0.push((target, "Unlocked".into())),
        Err(k2_world::craft::CraftError::CheckFailed { dc, total }) => {
            info!("security check failed: {total} vs DC {dc}");
            barks.0.push((target, "Security failed".into()));
        }
        Err(k2_world::craft::CraftError::KeyRequired) => barks.0.push((target, "Key required".into())),
        Err(_) => w.use_object(leader, target),
    }
}

fn use_with_feedback(w: &mut World, barks: &mut PendingBarks, leader: ObjectId, target: ObjectId) {
    info!("use {}", w.describe(target));
    let locked = w.get(target).is_some_and(|o| o.door().is_some_and(|d| d.locked) || o.placeable().is_some_and(|p| p.locked));
    w.use_object(leader, target);
    if locked && w.conversation.is_none() {
        barks.0.push((target, "Locked".into()));
    }
}

fn root_rotation(bearing: f32) -> Quat {
    Quat::from_rotation_z(bearing)
}

/// Spawns entities for new world objects, despawns destroyed ones, follows
/// positions, and keeps the `Player` marker on the party leader.
#[allow(clippy::too_many_arguments)]
fn sync_objects(
    mut commands: Commands,
    time: Res<Time>,
    world: Res<WorldRes>,
    game: Res<GameRes>,
    mut cache: ResMut<AssetCache>,
    mut images: ResMut<Assets<Image>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<KMaterial>>,
    mut ents: ResMut<ObjEntities>,
    area: Query<Entity, With<AreaRoot>>,
    mut stunts: Query<(&StuntOrigin, &mut Visibility)>,
    mut roots: Query<MirrorRoot>,
    gate: Res<crate::presentation::Gate>,
) {
    let Ok(area) = area.single() else { return };
    let w = &world.0;
    let leader = w.party_leader();
    let mut loader = Loader { res: &game.0, cache: &mut cache, images: &mut images, meshes: &mut meshes, materials: &mut materials };

    // Spawn.
    for o in w.iter() {
        if ents.0.contains_key(&o.id) {
            continue;
        }
        let Some(spec) = object_model(o) else { continue };
        let xf = Transform::from_translation(o.position).with_rotation(root_rotation(o.bearing));
        let root = commands.spawn((Name::new(format!("{:?} {}", o.kind, o.tag)), ObjId(o.id), xf, Visibility::default(), ChildOf(area))).id();
        let body = spawn_creature(&mut commands, &mut loader, &spec, root, Transform::IDENTITY);
        if body.is_none() {
            warn!("{}: model {} not loadable", w.describe(o.id), spec.body);
        }
        commands.entity(root).insert(ObjBody(body));
        // Placeables are solid scenery for the follow camera, like rooms.
        if o.kind == ObjectKind::Placeable {
            commands.entity(root).insert(play::RoomModel);
        }
        if o.creature().is_some_and(|c| c.hologram) {
            commands.entity(root).insert(crate::holo::HologramLook);
        }
        // Stunt clips key `cutscenedummy` at absolute area coordinates (the
        // 101per Kreia corpse: (-21.99, 12.30, 9.11), where the placeable
        // also sits). The pose is authored in area space, so the object root
        // stays at the area origin instead of adding its own transform. Many
        // ordinary placeables (plc_desk1, ...) also carry a `cutscenedummy`
        // node in `default`, keyed at the origin (identity); only a non-zero
        // key marks an area-space pose, else the object would collapse to
        // the area origin and vanish.
        if spec.own_idle
            && AnimSet::resolve(&mut loader, &spec.body)
                .and_then(|s| s.own_loop())
                .and_then(|c| c.tracks.get("cutscenedummy").map(|t| t.pos.iter().any(|(_, p)| p.length() > 0.01)))
                .unwrap_or(false)
        {
            commands.entity(root).insert((AreaSpacePose, StuntOrigin(o.position), Transform::IDENTITY));
        }
        if let Some(it) = interactable(w, o, leader) {
            commands.entity(root).insert(it);
        }
        match o.kind {
            ObjectKind::Creature => {
                let c = o.creature();
                commands.entity(root).insert(CreatureAnim {
                    walk_dist: c.map_or(1.75, |c| c.walk_speed),
                    run_dist: c.map_or(5.4, |c| c.run_speed),
                    ..default()
                });
            }
            ObjectKind::Door if o.door().is_some_and(|d| d.open) => {
                if let Some(b) = body {
                    let two = o.door().is_some_and(|d| d.side == 2);
                    commands.entity(b).insert(InitialClip(if two { "opened2" } else { "opened1" }));
                }
            }
            ObjectKind::Placeable if o.placeable().is_some_and(|p| p.open) => {
                if let Some(b) = body {
                    commands.entity(b).insert(InitialClip("open"));
                }
            }
            _ => {}
        }
        ents.0.insert(o.id, root);
    }

    // Despawn and follow.
    let dt = time.delta_secs();
    let k = 1.0 - (-14.0 * dt).exp();
    // In conversations the world owns the leader's bearing (dialog facing),
    // so the model follows it instead of local input.
    let controlled = w.player_controlled() && w.conversation.is_none();
    for (e, id, mut t, player, anim, area_space) in &mut roots {
        let Some(o) = w.get(id.0) else {
            // A death or knock-down whose hit has not landed keeps its body (presentation.rs).
            if gate.holds_pose(id.0) {
                continue;
            }
            commands.entity(e).despawn();
            ents.0.remove(&id.0);
            continue;
        };
        let is_leader = id.0 == leader;
        match (is_leader, player.is_some()) {
            (true, false) => {
                let facing = o.bearing + FRAC_PI_2;
                commands.entity(e).insert((Player { pos: o.position, facing }, PlayerMoveState { running: true, ..default() })).remove::<Interactable>();
            }
            (false, true) => {
                commands.entity(e).remove::<(Player, PlayerMoveState)>();
                if let Some(it) = interactable(w, o, leader) {
                    commands.entity(e).insert(it);
                }
            }
            _ => {}
        }
        let before = t.translation;
        if let Some(mut p) = player
            && is_leader
        {
            if !controlled {
                // Scripted: follow the world.
                p.pos = o.position;
                p.facing = o.bearing + FRAC_PI_2;
                follow(&mut t, o.position, o.bearing, k);
            }
        } else if area_space {
            *t = Transform::IDENTITY;
            if let Ok((origin, mut v)) = stunts.get_mut(e) {
                let want = if stunt_displaced(o.position, origin.0) { Visibility::Hidden } else { Visibility::Inherited };
                if *v != want {
                    *v = want;
                }
            }
        } else if o.kind == ObjectKind::Creature || o.kind == ObjectKind::Placeable || o.kind == ObjectKind::Door {
            follow(&mut t, o.position, o.bearing, k);
        }
        if let Some(mut a) = anim {
            let moved = (t.translation - a.prev.unwrap_or(before)).truncate().length();
            let inst = if dt > 0.0 { moved / dt } else { 0.0 };
            a.speed += (inst - a.speed) * (1.0 - (-10.0 * dt).exp());
            a.prev = Some(t.translation);
        }
    }
}

/// Object roots as `sync_objects` updates them.
type MirrorRoot = (
    Entity,
    &'static ObjId,
    &'static mut Transform,
    Option<&'static mut Player>,
    Option<&'static mut CreatureAnim>,
    Has<AreaSpacePose>,
);

/// The object's animation positions it in area space (stunt models); its
/// root stays at the area origin.
#[derive(Component)]
struct AreaSpacePose;

/// Where a stunt object was when its entity was made. A script that moves it
/// away afterwards (101per: a_cut_kreia jumps the get-up Kreia to MN_MEDLAB
/// once the standing Kreia exists) removes it from the scene: its pose is in
/// area space and does not move with the root, so it is hidden instead.
#[derive(Component)]
struct StuntOrigin(Vec3);

/// The stunt object was moved off its stage.
fn stunt_displaced(pos: Vec3, origin: Vec3) -> bool {
    pos.distance(origin) > 1.0
}

fn follow(t: &mut Transform, pos: Vec3, bearing: f32, k: f32) {
    if t.translation.distance(pos) > 3.0 {
        t.translation = pos;
    } else {
        t.translation = t.translation.lerp(pos, k);
    }
    t.rotation = t.rotation.slerp(root_rotation(bearing), k);
}

/// Clip to hold right after spawn (doors/placeables already open).
#[derive(Component)]
struct InitialClip(&'static str);

/// Animation clip candidates for an ANIMATION_* constant.
pub fn anim_clips(anim: i32) -> &'static [&'static str] {
    match anim {
        0 | 10000 => &["pause1"],
        1 => &["pause2", "pause1"],
        2 => &["listen", "pause1"],
        3 => &["meditate"],
        4 => &["meditate"],
        5 => &["tlknorm", "talk"],
        6 => &["tlkplead", "talk"],
        7 => &["tlkforce", "talk"],
        8 => &["tlklaugh", "talk"],
        9 => &["tlksad", "talk"],
        10 => &["getfromgnd", "kneel"],
        11 => &["getfromtbl"],
        12 => &["pausetrd", "pause1"],
        13 => &["pausetrd", "pause1"],
        14 => &["flirt"],
        15 => &["usecomplp", "usecomp"],
        16 => &["dance"],
        17 => &["dance1", "dance"],
        18 => &["horror"],
        19 => &["ready", "g1r1", "pause1"],
        20 => &["deactivate", "cpause1"],
        21 => &["spasm"],
        22 => &["sleep"],
        23 => &["prone"],
        24 => &["pause3", "pause1"],
        25 => &["weld", "unlockcntr"],
        26 => &["dead", "dead1"],
        27 => &["talkinj", "tlknorm"],
        28 => &["listeninj", "listen"],
        29 => &["treatinjlp", "treatinj"],
        // DEAD_PRONE: on the back, head opposite the facing (the 001ebo
        // medbay PC). dead/dead1 lie face down with the head the other way.
        30 => &["prone", "dead1", "dead"],
        31..=33 => &["kneel"],
        34 => &["unlockdr"],
        35 => &["meditatesit", "meditate"],
        // Sit-downs end seated. sitdown is the chair sit (rootdummy ends 0.42 m
        // down). Female bodies inherit it through S_Female01 > S_Male02; their
        // own "sit" is a lower floor pose, so it is not used. One-shot, held at
        // its last frame (scripted_playback).
        36..=40 => &["sitdown"],
        41 => &["forcerage"],
        42 => &["diveroll"],
        43 => &["closed"],
        44 => &["stealth"],
        45 => &["choke"],
        46 => &["meditatestand", "meditate"],
        100 => &["hturnl"],
        101 => &["hturnr"],
        102 => &["pausesh"],
        103 => &["pausebrd"],
        104 => &["salute"],
        105 => &["bow"],
        106 => &["greeting"],
        107 => &["taunt"],
        108..=110 => &["victory"],
        111 => &["pausebrd"],
        112 => &["inject"],
        113 => &["usecomp"],
        114 => &["persuade"],
        115 => &["activate"],
        116 => &["choke"],
        117 => &["throwgren"],
        118 => &["throwgren1", "throwgren"],
        120 => &["treatinj"],
        121 => &["castout1"],
        122 => &["open", "activate"],
        123 => &["diveroll"],
        124 => &["horror"],
        200 => &["on", "activate", "off2on"],
        201 => &["off", "deactivate", "on2off"],
        202 => &["close2open", "open"],
        203 => &["open2close", "close"],
        204 => &["animloop01"],
        205 => &["animloop02"],
        206 => &["animloop03"],
        207 => &["animloop04"],
        208 => &["animloop05"],
        209 => &["animloop06"],
        210 => &["animloop07"],
        211 => &["animloop08"],
        212 => &["animloop09"],
        213 => &["animloop10"],
        _ => &[],
    }
}

/// Death clip and the corpse loop that follows it.
const DEATH_CLIPS: [(&str, &str); 4] = [("die", "dead"), ("die1", "dead1"), ("die3", "dead3"), ("cdie", "cdead")];

fn pick_clip(player: &AnimPlayer, names: &[&str]) -> Option<String> {
    first_clip(&player.set, names)
}

/// The first name in `names` that the model's supermodel chain has.
fn first_clip(set: &AnimSet, names: &[&str]) -> Option<String> {
    names.iter().find(|n| set.has(n)).map(|n| (*n).to_owned())
}

/// Looping flag and deadline (None: until the next event) for a scripted
/// animation. A sit-down (ANIMATION_LOOPING_SIT_*) plays once and holds its
/// seated pose: looped, its clip snapped back up and sank again every clip
/// length, the "sitting down in a loop" the player saw.
fn scripted_playback(animation: i32, looping: bool, duration: f32, now: f32) -> (bool, Option<f32>) {
    if (36..=40).contains(&animation) {
        return (false, Some(if duration > 0.0 { now + duration } else { f32::INFINITY }));
    }
    // nwscript: the duration is not used for fire-and-forget animations.
    (looping, (looping && duration > 0.0).then(|| now + duration))
}

/// Door, placeable and scripted-animation events.
#[allow(clippy::too_many_arguments)]
fn apply_events(
    mut commands: Commands,
    time: Res<Time>,
    world: Res<WorldRes>,
    frames: Res<bevy::diagnostic::FrameCount>,
    events: Res<FrameEvents>,
    ents: Res<ObjEntities>,
    mut roots: Query<(&ObjBody, Option<&mut CreatureAnim>)>,
    mut players: Query<&mut AnimPlayer>,
    initial: Query<(Entity, &InitialClip)>,
    mut opening_doors: Local<Vec<ObjectId>>,
) {
    opening_doors.retain(|&d| {
        let waiting = world.0.get(d).and_then(|o| o.door()).is_some_and(|s| s.open && s.opening > 0.0);
        if !waiting && world.0.get(d).and_then(|o| o.door()).is_some_and(|s| s.open) {
            info!("door {} passable at frame {}", world.0.describe(d), frames.0);
        }
        waiting
    });
    for (e, c) in &initial {
        if let Ok(mut p) = players.get_mut(e) {
            if !p.play(c.0, true, 1.0) && c.0 == "opened2" {
                p.play("opened1", true, 1.0);
            }
        }
        commands.entity(e).remove::<InitialClip>();
    }
    for ev in &events.0 {
        let (obj, oneshot, hold): (ObjectId, &[&str], &[&str]) = match ev {
            // Side 2 (opened from the other side) plays the "2" clips; the
            // world waits opening<side> before the walkmesh opens (doors.rs).
            WorldEvent::DoorChanged { door, open } => {
                let st = world.0.get(*door).and_then(|o| o.door());
                let two = st.is_some_and(|d| d.side == 2);
                info!(
                    "door {} {} at frame {} (side {}, passable in {:.2} s)",
                    world.0.describe(*door),
                    if *open { "opening" } else { "closing" },
                    frames.0,
                    st.map_or(0, |d| d.side),
                    st.map_or(0.0, |d| d.opening),
                );
                if *open && st.is_some_and(|d| d.opening > 0.0) {
                    opening_doors.push(*door);
                }
                match (*open, two) {
                    (true, false) => (*door, &["opening1"], &["opened1"]),
                    (true, true) => (*door, &["opening2", "opening1"], &["opened2", "opened1"]),
                    (false, false) => (*door, &["closing1"], &["closed"]),
                    (false, true) => (*door, &["closing2", "closing1"], &["closed"]),
                }
            }
            WorldEvent::PlaceableChanged { placeable, open: true } => (*placeable, &["close2open"], &["open"]),
            WorldEvent::PlaceableChanged { placeable, open: false } => (*placeable, &["open2close"], &["close"]),
            WorldEvent::AttackStarted { attacker, anim, fallbacks, .. } => {
                let Some(&root) = ents.0.get(attacker) else { continue };
                let Ok((body, ca)) = roots.get_mut(root) else { continue };
                let (Some(b), Some(mut a)) = (body.0, ca) else { continue };
                let Ok(mut p) = players.get_mut(b) else { continue };
                let names: Vec<&str> = std::iter::once(anim.as_str()).chain(fallbacks.iter().map(String::as_str)).collect();
                if let Some(clip) = pick_clip(&p, &names) {
                    p.fade_to(&clip, false, 1.0, 0.1);
                    a.scripted = Some((clip, false, None));
                }
                continue;
            }
            // A knocked-out party member plays the same fall; it stays down
            // (the dead loop) until GotUp.
            WorldEvent::Died { object, .. } | WorldEvent::KnockedDown { object, .. } => {
                info!("died or knocked down: {} at frame {}", world.0.describe(*object), frames.0);
                let Some(&root) = ents.0.get(object) else { continue };
                let Ok((body, ca)) = roots.get_mut(root) else { continue };
                if let Some(mut a) = ca {
                    a.scripted = None;
                }
                let Some(b) = body.0 else { continue };
                let Ok(mut p) = players.get_mut(b) else { continue };
                // Humanoids have die/die1, monsters and droids cdie. animate_creatures
                // plays the matching dead loop once the clip ends.
                let names: &[&str] = if *object % 2 == 0 { &["die", "die1", "cdie"] } else { &["die1", "die", "cdie"] };
                if let Some(clip) = pick_clip(&p, names) {
                    p.fade_to(&clip, false, 1.0, 0.1);
                }
                continue;
            }
            // Gets up from the matching corpse pose (dead / dead1). A model
            // without a get-up clip just goes back to idle.
            WorldEvent::GotUp { object } => {
                info!("gets up: {} at frame {}", world.0.describe(*object), frames.0);
                let Some(&root) = ents.0.get(object) else { continue };
                let Ok((body, ca)) = roots.get_mut(root) else { continue };
                let (Some(b), Some(mut a)) = (body.0, ca) else { continue };
                let Ok(mut p) = players.get_mut(b) else { continue };
                let names: &[&str] = if p.current() == Some("dead1") {
                    &["getupdead1", "getupdead", "cgustandb"]
                } else {
                    &["getupdead", "getupdead1", "cgustandb"]
                };
                if let Some(clip) = pick_clip(&p, names) {
                    p.fade_to(&clip, false, 1.0, 0.1);
                    a.scripted = Some((clip, false, None));
                }
                continue;
            }
            WorldEvent::PlayAnimation { object, animation, speed, duration, looping } => {
                let Some(&root) = ents.0.get(object) else { continue };
                // kill() queues the dead loop right behind Died; the death clip
                // plays first and animate_creatures settles into the loop.
                if *animation == 26 && world.0.get(*object).and_then(|o| o.creature()).is_some_and(|c| c.dead) {
                    continue;
                }
                let Ok((body, ca)) = roots.get_mut(root) else { continue };
                let Some(b) = body.0 else { continue };
                let Ok(mut p) = players.get_mut(b) else { continue };
                let Some(clip) = pick_clip(&p, anim_clips(*animation)) else {
                    debug!("no clip for animation {animation}");
                    continue;
                };
                let speed = if *speed > 0.0 { *speed } else { 1.0 };
                let (looping, until) = scripted_playback(*animation, *looping, *duration, time.elapsed_secs());
                if let Some(mut a) = ca {
                    a.scripted = Some((clip.clone(), looping, until));
                    p.fade_to(&clip, looping, speed, 0.2);
                } else {
                    p.play(&clip, looping, speed);
                }
                continue;
            }
            _ => continue,
        };
        let Some(&root) = ents.0.get(&obj) else { continue };
        let Ok((body, _)) = roots.get(root) else { continue };
        let Some(b) = body.0 else { continue };
        let Ok(mut p) = players.get_mut(b) else { continue };
        if let Some(clip) = pick_clip(&p, oneshot) {
            p.play(&clip, false, 1.0);
            if let Some(h) = pick_clip(&p, hold) {
                commands.entity(b).insert(HoldAfter(h));
            }
        } else if let Some(h) = pick_clip(&p, hold) {
            p.play(&h, true, 1.0);
        }
    }
}

/// Test hooks. `K2_KILL=TEMPLATE[,FRAMES]` spawns TEMPLATE a few metres from
/// the leader (or at x,y,z when given) and kills it FRAMES (default 120)
/// frames later: `K2_KILL=g_sithtroop01,60,-8.7,-23.4,9.05`.
/// `K2_KILL_LOOT=ITEM` also gives the victim an item and opens the corpse.
/// `K2_DEATH_SHOT=PATH[,FRAMES...]` frames the first death and screenshots at
/// each FRAMES count after it (default 20; `{}` in PATH becomes the count),
/// then exits (use a fresh PATH each run).
#[allow(clippy::too_many_arguments)]
fn death_shot(
    mut commands: Commands,
    events: Res<FrameEvents>,
    mut world: ResMut<WorldRes>,
    mut exit: MessageWriter<bevy::app::AppExit>,
    player: Query<&mut Player>,
    mut cams: Query<(&mut Transform, Option<&mut crate::flycam::FlyCam>), With<Camera3d>>,
    mut left: Local<Option<u32>>,
    mut shot: Local<bool>,
    mut kill: Local<(u32, Option<ObjectId>)>,
    mut frame: Local<u32>,
    mut victim_id: Local<Option<ObjectId>>,
    ents: Res<ObjEntities>,
    inter: Query<(), With<Interactable>>,
) {
    let w = &mut world.0;
    *frame += 1;
    if let Ok(spec) = std::env::var("K2_KILL")
        && w.player_controlled()
        && let Ok(p) = player.single()
    {
        let mut parts = spec.split(',');
        let template = parts.next().unwrap_or_default();
        let delay: u32 = parts.next().and_then(|d| d.parse().ok()).unwrap_or(120);
        let fixed: Vec<f32> = parts.filter_map(|v| v.parse().ok()).collect();
        kill.0 += 1;
        if kill.1.is_none() && kill.0 == 5 {
            for o in w.iter().filter(|o| o.creature().is_some()).take(40) {
                info!("K2_KILL scene: {} at {:?} dead {}", w.describe(o.id), o.position, o.creature().is_some_and(|c| c.dead));
            }
            info!("K2_KILL leader at {:?}", p.pos);
            let a = p.facing;
            let at = p.pos + Vec3::new(a.cos(), a.sin(), 0.0) * 3.0;
            let at = if let [x, y, z] = fixed[..] { Vec3::new(x, y, z) } else { w.walk.ground(at.x, at.y, at.z + 1.5).map_or(at, |h| Vec3::new(at.x, at.y, h.z)) };
            match w.spawn_creature(template, at, a + std::f32::consts::PI - FRAC_PI_2) {
                Ok(id) => {
                    kill.1 = Some(id);
                    // K2_KILL_PARTY=member|leader: the victim joins the party first.
                    if let Ok(mode) = std::env::var("K2_KILL_PARTY") {
                        w.party.push(id);
                        if mode == "leader" {
                            w.set_party_leader(id);
                        }
                    }
                    if let Ok(item) = std::env::var("K2_KILL_LOOT") {
                        info!("K2_KILL_LOOT {item}: {:?}", w.create_item(&item, id, 1));
                    }
                }
                Err(e) => warn!("K2_KILL {template}: {e}"),
            }
        }
        if kill.0 == 5 + delay
            && let Some(id) = kill.1
        {
            let leader = w.party_leader();
            w.hurt_creature(id, i32::MAX / 2, leader);
        }
    }
    let Ok(spec) = std::env::var("K2_DEATH_SHOT") else { return };
    let mut parts = spec.split(',');
    let path = parts.next().unwrap_or_default().to_owned();
    let mut delays: Vec<u32> = parts.filter_map(|d| d.parse().ok()).collect();
    if delays.is_empty() {
        delays.push(20);
    }
    let last = delays.iter().copied().max().unwrap_or(20);
    let mut died = events.0.iter().find_map(|e| match e {
        WorldEvent::Died { object, .. } => Some(*object),
        _ => None,
    });
    // K2_SHOT_DEAD=1: frame an existing corpse (save load, GIT) after 120 frames.
    if died.is_none() && left.is_none() && *frame >= 120 && w.player_controlled() && std::env::var_os("K2_SHOT_DEAD").is_some() {
        let leader = w.party_leader();
        died = w.iter().find(|o| o.id != leader && o.creature().is_some_and(|c| c.dead)).map(|o| o.id);
        // GIT corpses are often live creatures or placeables posed by script.
        let body = |o: &Object| {
            let d = format!("{} {}", o.tag, w.describe(o.id)).to_ascii_lowercase();
            ["corpse", "dead", "body", "victim"].iter().any(|k| d.contains(k))
        };
        if died.is_none() && let Some(o) = w.iter().find(|o| o.id != leader && body(o)) {
            info!("K2_SHOT_DEAD: posed corpse {}", w.describe(o.id));
            died = Some(o.id);
        }
    }
    if died.is_some() {
        *victim_id = died;
    }
    if left.is_none()
        && let Some(victim) = died
    {
        *left = Some(0);
        // Frame the victim from its side (use with --fly, so nothing moves the camera).
        if let Some(o) = w.get(victim) {
            // K2_DEATH_CAM=DEGREES,METRES: where around the victim the camera stands.
            let cam: Vec<f32> = std::env::var("K2_DEATH_CAM").map(|v| v.split(',').filter_map(|x| x.parse().ok()).collect()).unwrap_or_default();
            let a = o.bearing + cam.first().map_or(FRAC_PI_2 + 0.5, |d| d.to_radians());
            info!("death shot: victim at {:?}", o.position);
            // Turn the camera around the victim until it stands on the walkmesh
            // with a clear line to the body, so walls never fill the shot.
            let dist = cam.get(1).copied().unwrap_or(3.2);
            let chest = o.position + Vec3::Z * 0.5;
            let eye = (0..16)
                .map(|k| a + k as f32 * std::f32::consts::TAU / 16.0)
                .map(|ang| o.position + Vec3::new(ang.cos(), ang.sin(), 0.0) * dist + Vec3::Z * 1.6)
                .find(|e| w.walk.ground(e.x, e.y, o.position.z + 1.0).is_some() && w.walk.line_of_sight(*e, chest))
                .unwrap_or_else(|| o.position + Vec3::new(a.cos(), a.sin(), 0.0) * dist + Vec3::Z * 1.6);
            for (mut t, fly) in &mut cams {
                *t = Transform::from_translation(crate::coords::to_bevy(eye)).looking_at(crate::coords::to_bevy(o.position + Vec3::Z * 0.5), Vec3::Y);
                if let Some(mut f) = fly {
                    let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
                    (f.yaw, f.pitch) = (yaw, pitch);
                }
            }
        }
    }
    if let Some(n) = &mut *left {
        if delays.contains(n) {
            if !*shot {
                *shot = true;
                if let Some(v) = *victim_id {
                    let leader = w.party_leader();
                    let dead = w.get(v).and_then(|o| o.creature()).is_some_and(|c| c.dead);
                    let marked = ents.0.get(&v).is_some_and(|e| inter.contains(*e));
                    info!("corpse check: dead {dead} container {} targetable {marked}", w.is_container(v));
                    if std::env::var_os("K2_KILL_LOOT").is_some() {
                        w.use_object(leader, v);
                        info!("corpse check: opened container {:?}", w.inventory.open_container);
                    }
                }
            }
            commands
                .spawn(bevy::render::view::screenshot::Screenshot::primary_window())
                .observe(bevy::render::view::screenshot::save_to_disk(std::path::PathBuf::from(path.replace("{}", &n.to_string()))));
        }
        if *n >= last && std::path::Path::new(&path.replace("{}", &last.to_string())).exists() {
            exit.write(bevy::app::AppExit::Success);
        }
        *n += 1;
    }
}

/// Corpses stay targetable while they carry loot and drop out otherwise;
/// a resurrected creature gets its marker back.
fn corpse_targets(mut commands: Commands, world: Res<WorldRes>, roots: Query<(Entity, &ObjId, Has<Interactable>), Without<Player>>) {
    let w = &world.0;
    let leader = w.party_leader();
    for (e, id, has) in &roots {
        let Some(o) = w.get(id.0) else { continue };
        let Some(c) = o.creature() else { continue };
        let want = if c.dead || c.down { w.is_container(id.0).then(|| interactable(w, o, leader)).flatten() } else { interactable(w, o, leader) };
        match (want, has) {
            (Some(it), false) => {
                commands.entity(e).insert(it);
            }
            (None, true) => {
                commands.entity(e).remove::<Interactable>();
            }
            _ => {}
        }
    }
}

fn settle_holds(mut commands: Commands, mut q: Query<(Entity, &HoldAfter, &mut AnimPlayer)>) {
    for (e, h, mut p) in &mut q {
        if p.finished() {
            p.play(&h.0, true, 1.0);
            commands.entity(e).remove::<HoldAfter>();
        }
    }
}

/// Creature animation: movement (player input or world motion) beats
/// scripted animations, which beat talking and idle.
fn animate_creatures(
    time: Res<Time>,
    world: Res<WorldRes>,
    mut roots: Query<(&ObjId, &ObjBody, &mut CreatureAnim, Option<&PlayerMoveState>)>,
    mut players: Query<&mut AnimPlayer>,
    gate: Res<crate::presentation::Gate>,
) {
    let w = &world.0;
    let now = time.elapsed_secs();
    let speaker = w.conversation_view().map(|v| v.speaker);
    for (id, body, mut a, st) in &mut roots {
        // A death whose hit has not landed yet keeps the pose it has (presentation.rs).
        if gate.holds_pose(id.0) {
            continue;
        }
        let Some(b) = body.0 else { continue };
        let Ok(mut p) = players.get_mut(b) else { continue };
        // Knocked out counts as lying down until it gets up.
        if w.get(id.0).and_then(|o| o.creature()).is_some_and(|c| c.dead || c.down) {
            let cur = p.current().map(str::to_owned);
            if let Some(&(_, hold)) = DEATH_CLIPS.iter().find(|(die, _)| cur.as_deref() == Some(*die)) {
                // Let the death clip finish, then hold its matching corpse loop
                // (or the last frame when the model has none).
                if p.finished() && p.set.has(hold) {
                    p.play(hold, true, 1.0);
                }
            } else if !DEATH_CLIPS.iter().any(|(_, hold)| cur.as_deref() == Some(*hold)) && cur.as_deref() != Some("prone") {
                // Dead without a death event (loaded from a save, spawned
                // dead): settle straight into the corpse pose.
                if let Some(hold) = pick_clip(&p, &["dead", "dead1", "cdead"]) {
                    p.play(&hold, true, 1.0);
                }
            }
            continue;
        }
        let (moving, running, speed) = match (st, w.motion.get(&id.0)) {
            (Some(st), _) if st.moving && st.speed > 0.2 => (true, st.world_driven.unwrap_or(st.running), st.speed),
            (_, Some(Motion::Walk)) => (true, false, a.speed),
            (_, Some(Motion::Run)) => (true, true, a.speed),
            _ => (false, false, 0.0),
        };
        if moving {
            a.scripted = None;
            let (name, dist) = if running { ("run", a.run_dist) } else { ("walk", a.walk_dist) };
            let len = p.set.clip(name).map_or(1.0, |c| c.length);
            let rate = if dist > 0.0 && len > 0.0 && speed > 0.05 { (speed * len / dist).clamp(0.3, 2.5) } else { 1.0 };
            p.fade_to(name, true, rate, 0.2);
            continue;
        }
        if let Some((clip, looping, until)) = &a.scripted {
            // A one-shot holds its last frame until its deadline (for ever when
            // the script gave no duration).
            let hold = !*looping && until.is_some_and(|u| u.is_infinite() || now < u);
            if hold && p.finished() {
                continue;
            }
            let done = if *looping { until.is_some_and(|u| now >= u) } else { p.finished() };
            if !done {
                let (clip, looping) = (clip.clone(), *looping);
                p.fade_to(&clip, looping, 1.0, 0.2);
                continue;
            }
            a.scripted = None;
        }
        // A stunt keeps its last pose (lying, then the end of the clip a
        // conversation played on it) instead of idling at the area origin.
        if w.get(id.0).and_then(|o| o.creature()).and_then(|c| c.body_model.as_deref()).is_some_and(is_stunt_model) {
            continue;
        }
        if speaker == Some(id.0) && p.set.has("tlknorm") {
            p.fade_to("tlknorm", true, 1.0, 0.3);
            continue;
        }
        // In combat a creature holds its ready stance for the weapon it wields
        // (g<family>r1, then the creature ready clips); out of combat it stands
        // at ease (pause1).
        if w.in_combat(id.0) {
            let ready = k2_world::combat::ready_clips(w.main_hand_family(id.0));
            if let Some(clip) = first_clip(&p.set, &ready.iter().map(String::as_str).collect::<Vec<_>>()) {
                p.fade_to(&clip, true, 1.0, 0.25);
                continue;
            }
        }
        for idle in IDLE_ANIMS {
            if p.set.has(idle) {
                p.fade_to(idle, true, 1.0, 0.25);
                break;
            }
        }
    }
}

/// F or left click on the hover target uses it (walking there first when
/// out of reach).
#[allow(clippy::too_many_arguments)]
fn interact(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    paused: Res<Paused>,
    conv: Res<InConversation>,
    target: Res<Target>,
    mut world: ResMut<WorldRes>,
    mut pending: ResMut<PendingUse>,
    mut verb: ResMut<PendingVerb>,
    mut barks: ResMut<PendingBarks>,
    mut clickwalk: ResMut<ClickWalk>,
    ids: Query<&ObjId>,
    player: Query<&Player>,
    ui: Query<&Interaction>,
) {
    let w = &mut world.0;
    if paused.0 || conv.0 || w.party_held() {
        // Only a dropped use forgets the walk's route: scripted leader moves run through here every frame.
        if pending.0.take().is_some() {
            w.forget_leader_path();
        }
        return;
    }
    let leader = w.party_leader();
    // Arrived at a pending target? Reach is the world's (k2-world reach_of).
    if let (Some(t), Ok(p)) = (pending.0, player.single()) {
        if w.get(t).is_none() {
            pending.0 = None;
        } else if w.in_use_reach(p.pos, t) {
            pending.0 = None;
            // The walk's route search latch goes too, so a later scripted move starts afresh.
            w.forget_leader_path();
            match std::mem::take(&mut *verb) {
                PendingVerb::Use => use_with_feedback(w, &mut barks, leader, t),
                PendingVerb::Unlock => unlock_with_feedback(w, &mut barks, leader, t),
            }
        }
    }
    let over_ui = ui.iter().any(|i| *i != Interaction::None);
    if !(keys.just_pressed(KeyCode::KeyF) || (buttons.just_pressed(MouseButton::Left) && !over_ui)) {
        return;
    }
    let Some(e) = target.0 else { return };
    let Ok(id) = ids.get(e) else { return };
    // Hostiles: attack (the world runs the leader into range).
    if w.get(id.0).is_some_and(|o| o.creature().is_some_and(|c| !c.dead)) && w.is_enemy(leader, id.0) {
        info!("attack {}", w.describe(id.0));
        pending.0 = None;
        w.player_attack(id.0);
        return;
    }
    // Locked doors and containers that can be bashed: attack them (the
    // plasma torch or a lightsaber cuts through Hardness; bash.rs).
    let locked = w.get(id.0).is_some_and(|o| o.door().is_some_and(|d| d.locked) || o.placeable().is_some_and(|p| p.locked));
    if locked && w.can_bash(id.0) {
        info!("bash {}", w.describe(id.0));
        pending.0 = None;
        w.player_attack(id.0);
        return;
    }
    let Ok(p) = player.single() else { return };
    click_object(w, &mut clickwalk, &mut pending, &mut verb, &mut barks, id.0, p.pos);
}

/// A click on `id` by the player at `from`: uses it when it is in reach, else
/// starts a walk to it. Either way an earlier walk ends, and a new walk starts
/// afresh (spot and stall clock included). `interact` and its test share this.
pub fn click_object(
    w: &mut World,
    clickwalk: &mut ClickWalk,
    pending: &mut PendingUse,
    verb: &mut PendingVerb,
    barks: &mut PendingBarks,
    id: ObjectId,
    from: Vec3,
) {
    if w.get(id).is_none() {
        return;
    }
    clickwalk.begin(w);
    if w.in_use_reach(from, id) {
        pending.0 = None;
        let leader = w.party_leader();
        use_with_feedback(w, barks, leader, id);
    } else {
        pending.0 = Some(id);
        *verb = PendingVerb::Use;
    }
}

/// `--autotalk TAG`, `--autotransition`, `--autouse SECS`: scripted test
/// input after an area load.
#[allow(clippy::too_many_arguments)]
fn auto_tests(
    time: Res<Time<Real>>,
    args: Res<Args>,
    target: Res<Target>,
    mut auto: ResMut<AutoState>,
    mut world: ResMut<WorldRes>,
    ids: Query<&ObjId>,
    mut players: Query<&mut Player>,
) {
    auto.since_load += time.delta_secs();
    let w = &mut world.0;
    let leader = w.party_leader();
    if let Some(tag) = &args.autotalk
        && !auto.talked
        && auto.since_load > 1.5
        && w.player_controlled()
    {
        auto.talked = true;
        match w.by_tag(tag, 0) {
            Some(npc) => {
                info!("autotalk: using {}", w.describe(npc));
                w.use_object(leader, npc);
            }
            None => warn!("autotalk: no object tagged {tag}"),
        }
    }
    if let Some(at) = args.autouse
        && !auto.used
        && auto.since_load > at
    {
        auto.used = true;
        match target.0.and_then(|e| ids.get(e).ok()) {
            Some(id) => {
                info!("autouse: {}", w.describe(id.0));
                w.use_object(leader, id.0);
            }
            None => info!("autouse: no target"),
        }
    }
    if args.autoattack && !auto.attacked && auto.since_load > 3.0 && w.player_controlled() {
        auto.attacked = true;
        let me = w.get(leader).map(|o| o.position).unwrap_or_default();
        let ids: Vec<(ObjectId, f32)> = w
            .iter()
            .filter(|o| o.creature().is_some_and(|c| !c.dead))
            .map(|o| (o.id, o.position.distance(me)))
            .collect();
        let mut hostile: Vec<(ObjectId, f32)> = ids.into_iter().filter(|(id, _)| w.is_enemy(leader, *id)).collect();
        hostile.sort_by(|a, b| a.1.total_cmp(&b.1));
        match hostile.first() {
            Some(&(id, d)) => {
                // Like the k2world fight harness: start 5 m away on open
                // ground with a clear line (there is no pathfinding yet).
                let t = w.get(id).map(|o| o.position).unwrap_or_default();
                let spot = (0..16).find_map(|k| {
                    let a = k as f32 / 16.0 * std::f32::consts::TAU;
                    let q = t + Vec3::new(a.cos(), a.sin(), 0.0) * 5.0;
                    let h = w.walk.ground(q.x, q.y, t.z + 1.5)?;
                    let q = Vec3::new(q.x, q.y, h.z);
                    w.walk.line_of_sight(q + Vec3::Z, t + Vec3::Z).then_some(q)
                });
                info!("autoattack: {} at {d:.1} m, starting from {spot:?}", w.describe(id));
                if let Some(q) = spot {
                    if let Some(o) = w.get_mut(leader) {
                        o.position = q;
                    }
                    if let Ok(mut p) = players.single_mut() {
                        p.pos = q;
                    }
                }
                w.player_attack(id);
            }
            None => warn!("autoattack: no hostile creature"),
        }
    }
    if let Some(tag) = &args.autoattack_door
        && auto.attacked
        && let Some(id) = w.by_tag(tag, 0)
        && let Some(d) = w.get(id).and_then(|o| o.door())
        && d.open
        && d.hp.destroyed
        && !auto.door_logged
    {
        auto.door_logged = true;
        info!("autoattack-door: {} open at {:.2} s since load, hp {:?}, locked {}", w.describe(id), auto.since_load, d.hp, d.locked);
    }
    if let Some(tag) = &args.autoattack_door
        && !auto.attacked
        && auto.since_load > 3.0
        && w.player_controlled()
    {
        auto.attacked = true;
        match w.by_tag(tag, 0) {
            Some(id) => {
                // Stand 2 m out on whichever side of it has ground.
                let (t, b) = w.get(id).map(|o| (o.position, o.bearing)).unwrap_or_default();
                let n = Vec3::new(-b.sin(), b.cos(), 0.0);
                let spot = [2.0f32, -2.0].into_iter().find_map(|s| {
                    let q = t + n * s;
                    w.walk.ground(q.x, q.y, t.z + 1.0).map(|h| Vec3::new(q.x, q.y, h.z))
                });
                info!("autoattack-door: {} hp {:?} bashable {} from {spot:?} at {:.2} s since load", w.describe(id), w.object_hp(id), w.can_bash(id), auto.since_load);
                if let Some(q) = spot {
                    if let Some(o) = w.get_mut(leader) {
                        o.position = q;
                    }
                    if let Ok(mut p) = players.single_mut() {
                        p.pos = q;
                    }
                }
                w.player_attack(id);
            }
            None => warn!("autoattack-door: no object tagged {tag}"),
        }
    }
    if args.autotransition && !auto.transitioned && auto.since_load > 1.5 && w.player_controlled() {
        auto.transitioned = true;
        let trig = w.iter().find_map(|o| {
            let t = o.trigger()?;
            (t.is_transition && t.linked_to_module.is_some() && !t.polygon.is_empty()).then(|| {
                let n = t.polygon.len() as f32;
                let c = t.polygon.iter().fold(Vec3::ZERO, |a, p| a + Vec3::from(*p)) / n;
                (o.tag.clone(), c, t.linked_to_module.clone())
            })
        });
        match trig {
            Some((tag, c, to)) => {
                let z = w.walk.ground(c.x, c.y, c.z + 2.0).map_or(c.z, |h| h.z);
                let pos = Vec3::new(c.x, c.y, z);
                info!("autotransition: stepping into trigger {tag} -> {to:?} at {pos}");
                if let Some(o) = w.get_mut(leader) {
                    o.position = pos;
                }
                if let Ok(mut p) = players.single_mut() {
                    p.pos = pos;
                }
            }
            None => warn!("autotransition: no transition trigger in this module"),
        }
    }
}

/// `--autoreply`: in conversations pick reply 0 every 1.5 s (exercises the
/// same path as the dialog UI).
pub fn autoreply_tick(dt: f32, args: &Args, world: &mut World, timer: &mut f32) -> bool {
    if !args.autoreply || world.conversation.is_none() {
        *timer = 0.0;
        return false;
    }
    *timer += dt;
    if *timer < 1.5 {
        return false;
    }
    *timer = 0.0;
    if let Some(v) = world.conversation_view()
        && !v.replies.is_empty()
        && !(v.replies.len() == 1 && v.replies[0].text.trim().is_empty())
    {
        info!("autoreply: {}", v.replies[0].text);
        world.choose_reply(0);
        return true;
    }
    false
}

/// Used by the dialog UI to share the autoreply timer.
#[derive(Resource, Default)]
pub struct AutoReplyTimer(pub f32);

#[cfg(test)]
mod stunt_tests;

#[cfg(test)]
mod sit_tests;

#[cfg(test)]
mod attack_clip_tests;


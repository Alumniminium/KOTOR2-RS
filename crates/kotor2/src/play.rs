//! Third-person play: the player on the area walkmesh and a follow camera.
//!
//! Controls: W/S forward/back, A/D turn (the camera turns with the
//! character), Q/E rotate the camera, right or middle mouse drag orbits it,
//! mouse wheel zooms, Shift toggles walk/run, F interacts, F1 toggles the
//! free-fly debug camera. The camera eases after the character and swings
//! behind it while it walks toward a clicked target.
//!
//! Positions here are game space (Z-up). The player entity lives under
//! `WorldRoot`, so its Transform is game space too; only the camera, which
//! sits outside the world root, converts with `coords::to_bevy`.

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::picking::mesh_picking::ray_cast::{MeshRayCast, MeshRayCastSettings, RayCastBackfaces, RayCastVisibility};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use k2_script::ObjectId;
use k2_walk::SpotSearch;
use k2_world::nav::UseStep;

use crate::Args;
use crate::coords;
use crate::flycam::FlyCam;
use crate::material::{Blend, KMaterial};
use crate::game::{GameSet, InConversation, MovieCards, Paused, PendingUse, WorldRes};

pub const RUN_SPEED: f32 = 5.4;
pub const WALK_SPEED: f32 = 1.75;
/// Camera look-at height above the player's feet.
const EYE: f32 = 1.5;
const TURN_RATE: f32 = 12.0;

/// The controlled character (the world's party leader). `pos` is game
/// space (feet), `facing` a game yaw (radians, 0 = +X, counter-clockwise
/// about +Z). World bearing = facing - 90 deg.
#[derive(Component)]
pub struct Player {
    pub pos: Vec3,
    pub facing: f32,
}

/// Movement state for animation. `speed` is the actual ground speed this
/// frame in m/s; `moving` is true while input moves the player or the world
/// walks it (a control walk); `running` is the walk/run toggle. `world_driven`
/// is Some(run) while the world walks the leader (a control walk), and None
/// while the player's own input moves it (the toggle applies then).
#[derive(Component, Default, Clone, Copy, PartialEq, Debug)]
pub struct PlayerMoveState {
    pub speed: f32,
    pub moving: bool,
    pub running: bool,
    pub world_driven: Option<bool>,
}

/// Follow camera state. `yaw` is the game yaw the camera looks along,
/// `pitch` > 0 looks down, `dist` is the wanted distance and `cur` the
/// distance actually used after collision.
#[derive(Component)]
pub struct PlayCam {
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub cur: f32,
    /// Extra pitch (rad) added while geometry behind the player would
    /// otherwise pull the camera into the head.
    pub lift: f32,
    /// The yaw actually drawn: eases toward `yaw`, which input and movement
    /// use directly.
    pub view_yaw: f32,
    /// Smoothed look-at point (feet of the character); `None` snaps to it.
    pub focus: Option<Vec3>,
}

impl Default for PlayCam {
    fn default() -> Self {
        Self { yaw: 0.0, pitch: 0.35, dist: 4.0, cur: 4.0, lift: 0.0, view_yaw: 0.0, focus: None }
    }
}

/// Root of a room model or placeable (geometry the follow camera collides with).
#[derive(Component)]
pub struct RoomModel;

/// A rendered room mesh; the follow camera ray-casts against these.
#[derive(Component)]
pub struct RoomMesh;

/// Tags the mesh entities of newly spawned rooms.
/// Translucent meshes (kolto tank glass, holograms) never block the camera:
/// the 101per leader starts inside a glass tank.
fn mark_room_meshes(
    mut commands: Commands,
    roots: Query<Entity, Added<RoomModel>>,
    children: Query<&Children>,
    meshes: Query<&MeshMaterial3d<KMaterial>, With<Mesh3d>>,
    materials: Res<Assets<KMaterial>>,
) {
    for r in &roots {
        for d in children.iter_descendants(r) {
            if meshes.get(d).is_ok_and(|m| materials.get(&m.0).is_some_and(|k| k.blend == Blend::Opaque)) {
                // Backfaces count: the leader can stand inside a placeable's
                // bounds (101per start), where every face looks away.
                commands.entity(d).insert((RoomMesh, RayCastBackfaces));
            }
        }
    }
}

/// Closest the follow camera prefers: blocked views lift the camera over the
/// player instead of zooming into the head, as the original does.
const CAM_MIN_DIST: f32 = 1.5;
/// Q/E camera rotation and A/D turn rates (rad/s).
const CAM_ORBIT_RATE: f32 = 2.2;
const CAM_TURN_RATE: f32 = 2.4;
/// Easing rates (1/s): swing behind a walking character, drawn yaw, focus. The swing is about
/// 90% round in 0.4 s and 95% in 0.5 s.
const CAM_FOLLOW_RATE: f32 = 6.0;
const CAM_VIEW_RATE: f32 = 14.0;
const CAM_FOCUS_RATE: f32 = 9.0;
/// Most the camera lifts above the player's chosen pitch.
const CAM_MAX_LIFT: f32 = 0.9;

#[derive(Resource, Clone, Copy, PartialEq, Eq, Debug)]
pub enum CamMode {
    Play,
    Fly,
}

pub struct PlayPlugin;

impl Plugin for PlayPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(CamMode::Play)
            .add_systems(Update, (toggle_mode, move_player).chain().in_set(GameSet::Input))
            .add_systems(Update, (mark_room_meshes, follow_camera, autowalk_log).chain().in_set(GameSet::Ui));
    }
}

/// The camera for play: follow cam, or the fly cam with `--fly` / `--cam`.
pub fn spawn_camera(commands: &mut Commands, args: &Args) {
    let fly = args.cam.is_some() || args.fly;
    let (pos, yaw, pitch) = match args.cam {
        Some([x, y, z, yaw, pitch]) => (Vec3::new(x, y, z), yaw.to_radians(), pitch.to_radians()),
        None => (Vec3::new(0.0, 0.0, 5.0), 0.0, 0.0),
    };
    let byaw = coords::game_yaw_to_bevy(yaw);
    let mut cam = commands.spawn((
        Camera3d::default(),
        Projection::Perspective(PerspectiveProjection { fov: 55f32.to_radians(), near: 0.05, far: 2000.0, ..default() }),
        Transform::from_translation(coords::to_bevy(pos)).with_rotation(Quat::from_euler(EulerRot::YXZ, byaw, pitch, 0.0)),
    ));
    if fly {
        cam.insert(FlyCam { yaw: byaw, pitch, speed: 6.0 });
    } else {
        let mut pc = PlayCam::default();
        if let Some((dist, _, pitch)) = cam_view_hook() {
            pc = PlayCam { pitch, dist, cur: dist, ..pc };
        }
        cam.insert(pc);
    }
    commands.insert_resource(if fly { CamMode::Fly } else { CamMode::Play });
}

/// Test hook: `K2_CAM_VIEW=DIST,YAW_DEG,PITCH_DEG` starts the follow camera at
/// that distance and pitch, YAW_DEG around from behind the leader (180 = the
/// front), for close-up screenshots.
fn cam_view_hook() -> Option<(f32, f32, f32)> {
    let v: Vec<f32> = std::env::var("K2_CAM_VIEW").ok()?.split(',').filter_map(|s| s.trim().parse().ok()).collect();
    (v.len() == 3).then(|| (v[0], v[1].to_radians(), v[2].to_radians()))
}

fn toggle_mode(
    mut commands: Commands,
    keys: Res<ButtonInput<KeyCode>>,
    mut mode: ResMut<CamMode>,
    cams: Query<(Entity, &Transform), With<Camera3d>>,
    player: Query<&Player>,
) {
    if !keys.just_pressed(KeyCode::F1) {
        return;
    }
    for (cam, t) in &cams {
        match *mode {
            CamMode::Play => {
                let (yaw, pitch, _) = t.rotation.to_euler(EulerRot::YXZ);
                commands.entity(cam).remove::<PlayCam>().insert(FlyCam { yaw, pitch, speed: 6.0 });
            }
            CamMode::Fly => {
                let yaw = player.single().map_or(0.0, |p| p.facing);
                commands.entity(cam).remove::<FlyCam>().insert(PlayCam { yaw, view_yaw: yaw, ..default() });
            }
        }
    }
    *mode = if *mode == CamMode::Play { CamMode::Fly } else { CamMode::Play };
    info!("camera mode {:?}", *mode);
}

/// Seconds since play started; drives `--autowalk`.
#[derive(Default)]
pub struct Clock(f32);

/// A click-walk that moves the leader less than STALL_MOVE metres a frame for
/// STALL_SECS is dropped (a route can end short of the object, and the leader
/// stands there). As tick.rs does for scripted moves.
const STALL_SECS: f32 = 1.5;
const STALL_MOVE: f32 = 1e-3;

/// Seconds the current click-walk has made no progress.
#[derive(Default)]
pub struct WalkStall(f32);

/// The click-walk in progress: the spot search for the clicked object, the
/// stand it found, and the stall clock. The search is one route search toward
/// all candidate spots (`World::begin_use_search`), stepped a frame at a time
/// (`World::step_use_search`), so a click costs one search spread over frames.
/// A new click resets it (`begin`).
#[derive(Default, Resource)]
pub struct ClickWalk {
    target: Option<ObjectId>,
    /// The spot search for `target`; None before it begins and after it ends.
    search: Option<SpotSearch>,
    stand: Option<Vec3>,
    stall: WalkStall,
}

impl ClickWalk {
    /// A new click on an object out of reach: the walk searches afresh.
    pub fn begin(&mut self, w: &mut k2_world::World) {
        *self = Self::default();
        w.forget_leader_path();
    }

    /// The direction the leader walks this frame toward the clicked object, or
    /// None when no walk runs: no click, the leader is already in use reach
    /// (the interact arrival uses it), a spot is still being searched, or the
    /// walk has just ended.
    pub fn heading(&mut self, w: &mut k2_world::World, pending: &mut PendingUse, from: Vec3) -> Option<Vec2> {
        let target = pending.0.filter(|&t| w.is_valid(t))?;
        if self.target != Some(target) {
            *self = Self { target: Some(target), ..Self::default() };
        }
        if w.in_use_reach(from, target) {
            return None;
        }
        if self.stand.is_none() {
            // One step of the spot search a frame; steering starts once it has a stand.
            let mut search = match self.search.take() {
                Some(search) => search,
                None => match w.begin_use_search(from, target) {
                    Some(search) => search,
                    None => {
                        end_walk(w, pending, self, from, target);
                        return None;
                    }
                },
            };
            match w.step_use_search(&mut search) {
                UseStep::Pending => self.search = Some(search),
                UseStep::Stand(at) => self.stand = Some(at),
                UseStep::NoStand => end_walk(w, pending, self, from, target),
            }
            return None;
        }
        let stand = self.stand?;
        match w.steer_leader(from, stand) {
            // The route is still being searched: the leader stands still, which is not walking,
            // so the stall clock does not run.
            Some(_) if w.leader_route_pending() => None,
            Some(aim) => {
                // A closed door on the leg, in reach: a locked one stops the walk (the use gives the
                // locked-door feedback); an unlocked one is opened, and the walk waits for the swing.
                if let Some(door) = w.door_across(from, aim)
                    && w.in_use_reach(from, door)
                {
                    let leader = w.party_leader();
                    let (locked, open) = w.get(door).and_then(|o| o.door()).map_or((false, false), |d| (d.locked, d.open));
                    if locked {
                        w.use_object(leader, door);
                        end_walk(w, pending, self, from, target);
                        return None;
                    }
                    if !open {
                        w.use_object(leader, door);
                    }
                    // Waits for the swing only; a door that is open and has swung is walked through, and one that did not open ends the walk.
                    let (open, swinging) = w.get(door).and_then(|o| o.door()).map_or((false, false), |d| (d.open, d.opening > 0.0));
                    if !open {
                        end_walk(w, pending, self, from, target);
                        return None;
                    }
                    if swinging {
                        return None;
                    }
                }
                Some((aim - from).truncate().normalize_or_zero())
            }
            None => {
                end_walk(w, pending, self, from, target);
                None
            }
        }
    }

    /// One frame of walking: the leader moved from `from` to `to` in `dt`
    /// seconds, `walking` while a click-walk steered it. A stalled walk ends,
    /// unless the step brought the leader into reach: the use then runs at
    /// the next frame's arrival check, so the pending use is left for it.
    pub fn progress(&mut self, w: &mut k2_world::World, pending: &mut PendingUse, from: Vec3, to: Vec3, walking: bool, dt: f32) {
        if self.stall.advance(walking, (to - from).truncate().length(), dt)
            && let Some(target) = self.target
            && !w.in_use_reach(to, target)
        {
            end_walk(w, pending, self, to, target);
        }
    }
}

/// Ends a click-walk, once: the pending use is dropped, and the game says the
/// object cannot be reached when the leader is still out of use reach.
fn end_walk(w: &mut k2_world::World, pending: &mut PendingUse, walk: &mut ClickWalk, from: Vec3, target: ObjectId) {
    pending.0 = None;
    *walk = ClickWalk::default();
    w.forget_leader_path();
    if !w.in_use_reach(from, target) {
        w.report_unreachable();
    }
}

impl WalkStall {
    /// Advances one frame. `walking` is true while a click-walk steers the
    /// leader; `moved` is the leader's step this frame. True when the walk has
    /// stalled (the clock restarts).
    pub fn advance(&mut self, walking: bool, moved: f32, dt: f32) -> bool {
        if !walking || moved >= STALL_MOVE {
            self.0 = 0.0;
            return false;
        }
        self.0 += dt;
        if self.0 < STALL_SECS {
            return false;
        }
        self.0 = 0.0;
        true
    }
}

/// Moves the party leader from keyboard input (or toward a pending use
/// target) on the world walkmesh and reports the position to the world.
/// While the world has control (scripts, dialog) the leader follows the
/// world instead (see `game::sync_objects`).
#[allow(clippy::too_many_arguments)]
pub fn move_player(
    time: Res<Time>,
    args: Res<Args>,
    keys: Res<ButtonInput<KeyCode>>,
    mode: Res<CamMode>,
    paused: Res<Paused>,
    conv: Res<InConversation>,
    cards: Res<MovieCards>,
    mut pending: ResMut<PendingUse>,
    mut world: ResMut<WorldRes>,
    cams: Query<&PlayCam>,
    mut players: Query<(&mut Player, &mut PlayerMoveState, &mut Transform)>,
    mut clock: Local<Clock>,
    mut clickwalk: ResMut<ClickWalk>,
) {
    let dt = time.delta_secs();
    let w = &mut world.0;
    let wants_move = [KeyCode::KeyW, KeyCode::KeyA, KeyCode::KeyS, KeyCode::KeyD].iter().any(|k| keys.pressed(*k));
    // Moving breaks off an attack (the leader's combat actions), as in the
    // original; scripted actions outside combat keep control.
    let leader = w.party_leader();
    if wants_move
        && !paused.0
        && !conv.0
        && (w.in_combat(leader) || w.is_bashing(leader))
        && let Some(o) = w.get_mut(leader).filter(|o| o.commandable)
    {
        o.actions.clear();
    }
    // A cutscene (a conversation, scripted input block or scripted leader actions) or a movie card holds the
    // party: no keyboard step, and a click walk in progress is dropped rather than resumed afterwards.
    let controlled = !paused.0 && !conv.0 && !w.party_held() && cards.current().is_none();
    if !controlled {
        *clickwalk = ClickWalk::default();
    }
    if controlled {
        clock.0 += dt;
    }
    let cam_yaw = cams.iter().next().map(|c| c.yaw);
    for (mut p, mut st, mut t) in &mut players {
        if keys.just_pressed(KeyCode::ShiftLeft) || keys.just_pressed(KeyCode::ShiftRight) {
            st.running = !st.running;
        }
        if !controlled {
            st.moving = false;
            st.speed = 0.0;
            st.world_driven = None;
            continue;
        }
        // The world owns where the leader stands: a walk or move made on the world since the last
        // frame (the control API's walks, a debug move) is where this frame starts. Without this the
        // world position is written back over it below every frame.
        let last = p.pos;
        if let Some(o) = w.get(leader) {
            p.pos = o.position;
            p.facing = o.bearing + std::f32::consts::FRAC_PI_2;
        }
        // Where the world walked the leader since the last frame, and how fast: a control walk
        // animates the leader like the player's own steps. A jump (a teleport) is not a walk.
        let world_speed = if dt > 0.0 { (p.pos - last).truncate().length() / dt } else { 0.0 };
        let world_walk = world_speed > 0.05 && world_speed < 2.0 * RUN_SPEED;
        let mut input = Vec2::ZERO; // x = forward
        if *mode == CamMode::Play {
            if keys.pressed(KeyCode::KeyW) {
                input.x += 1.0;
            }
            if keys.pressed(KeyCode::KeyS) {
                input.x -= 1.0;
            }
            if let Some((secs, _)) = args.autowalk
                && clock.0 < secs
            {
                input.x = 1.0;
            }
        }
        let yaw = cam_yaw.unwrap_or(p.facing);
        let fwd = Vec2::new(yaw.cos(), yaw.sin());
        let mut dir = (fwd * input.x).normalize_or_zero();
        // Walking to a clicked object (ClickWalk): to a spot within its use
        // reach, round walls and closed doors (nav.rs). A walk that cannot get
        // there ends with the game's "Unable to reach target!".
        let mut walking = false;
        if dir == Vec2::ZERO
            && let Some(heading) = clickwalk.heading(w, &mut pending, p.pos)
        {
            dir = heading;
            walking = true;
        }
        let speed = if st.running { RUN_SPEED } else { WALK_SPEED };
        let from = p.pos;
        let mut pos = if dir != Vec2::ZERO { w.player_move(from, dir * speed * dt, w.leader_clearance()) } else { from };
        if let Some(h) = w.walk.ground(pos.x, pos.y, pos.z) {
            pos.z = h.z;
        }
        clickwalk.progress(w, &mut pending, from, pos, walking, dt);
        p.pos = pos;
        // The player's own steps (keyboard, click) or the world's walk (a control walk). A world
        // walk turns the leader already: its facing is the walk's bearing, synced above.
        let own = dir != Vec2::ZERO;
        st.moving = own || world_walk;
        st.speed = if own {
            if dt > 0.0 { (pos - from).truncate().length() / dt } else { 0.0 }
        } else if world_walk {
            world_speed
        } else {
            0.0
        };
        st.world_driven = (!own && world_walk).then_some(world_speed > (WALK_SPEED + RUN_SPEED) / 2.0);
        let turning = *mode == CamMode::Play && (keys.pressed(KeyCode::KeyA) != keys.pressed(KeyCode::KeyD));
        if own || turning {
            // Turning in place follows the camera heading.
            let want = if own { dir.y.atan2(dir.x) } else { yaw };
            let diff = (want - p.facing + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            p.facing += diff.clamp(-TURN_RATE * dt, TURN_RATE * dt);
            p.facing = (p.facing + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        }
        // Model faces +Y at rest: world bearing = facing - 90 deg.
        let bearing = p.facing - std::f32::consts::FRAC_PI_2;
        t.translation = p.pos;
        t.rotation = Quat::from_rotation_z(bearing);
        w.set_player_position(p.pos, bearing);
    }
}

/// The camera yaw after `dt` with the leader's movement: while the leader walks (keys, click or a control
/// walk) the yaw eases round to the leader's facing, so the camera sits behind it, with no snap. It stays
/// where the player left it when the leader stands still, when the player turns or orbits it (`manual`),
/// and while the leader walks backward (S), whose facing would turn the input round with the camera.
fn camera_swing(yaw: f32, facing: f32, moving: bool, manual: bool, walking_back: bool, dt: f32) -> f32 {
    if !moving || manual || walking_back {
        return yaw;
    }
    let diff = (facing - yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
    yaw + diff * (1.0 - (-CAM_FOLLOW_RATE * dt).exp())
}

#[allow(clippy::too_many_arguments)]
fn follow_camera(
    time: Res<Time>,
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    windows: Query<&Window, With<PrimaryWindow>>,
    mut last_cursor: Local<Option<Vec2>>,
    conv: Res<InConversation>,
    paused: Res<Paused>,
    state: Query<(&Player, &PlayerMoveState)>,
    player: Query<&Transform, With<Player>>,
    mut player_vis: Query<&mut Visibility, With<Player>>,
    mut cams: Query<(&mut Transform, &mut PlayCam), Without<Player>>,
    mut ray: MeshRayCast,
    rooms: Query<(), With<RoomMesh>>,
) {
    if conv.0 {
        return;
    }
    // The rendered position (smoothed while scripts move the leader).
    let Ok(pt) = player.single() else { return };
    let ppos = pt.translation;
    let dt = time.delta_secs();
    let room_filter = |e: Entity| rooms.contains(e);
    let settings = MeshRayCastSettings::default().with_visibility(RayCastVisibility::Any).with_filter(&room_filter);
    // Mouse drag: cursor movement (works wherever the window sees the
    // cursor), else raw device motion.
    let cursor = windows.single().ok().and_then(|w| w.cursor_position());
    let cursor_delta = match (cursor, *last_cursor) {
        (Some(c), Some(l)) => c - l,
        _ => Vec2::ZERO,
    };
    *last_cursor = cursor;
    let drag = if cursor_delta != Vec2::ZERO { cursor_delta } else { motion.delta };
    let dragging = buttons.pressed(MouseButton::Right) || buttons.pressed(MouseButton::Middle);
    let walking_back = keys.pressed(KeyCode::KeyS);
    let (moving, facing) = state.single().map_or((false, 0.0), |(p, st)| (st.moving, p.facing));
    for (mut t, mut cam) in &mut cams {
        let mut manual = false;
        if dragging {
            cam.yaw -= drag.x * 0.004;
            cam.pitch = (cam.pitch + drag.y * 0.003).clamp(-0.4, 1.3);
            manual = drag != Vec2::ZERO;
        }
        if !paused.0 {
            // Q/E orbit the camera; A/D turn the character and camera together.
            let spin = |neg: KeyCode, pos: KeyCode| f32::from(keys.pressed(pos)) - f32::from(keys.pressed(neg));
            let mut q = spin(KeyCode::KeyE, KeyCode::KeyQ);
            // Test hook: `K2_CAM_SPIN=SECS` holds Q for SECS seconds from the start.
            if let Some(secs) = std::env::var("K2_CAM_SPIN").ok().and_then(|v| v.parse::<f32>().ok())
                && time.elapsed_secs() > 8.0
                && time.elapsed_secs() < 8.0 + secs
            {
                q = 1.0;
            }
            let a = spin(KeyCode::KeyD, KeyCode::KeyA);
            cam.yaw += (q * CAM_ORBIT_RATE + a * CAM_TURN_RATE) * dt;
            manual |= q != 0.0 || a != 0.0;
            // The leader walks (keys, click or a control walk): swing round behind it.
            cam.yaw = camera_swing(cam.yaw, facing, moving, manual, walking_back, dt);
        }
        cam.yaw = (cam.yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        // The drawn yaw eases after the wanted one; the focus after the
        // character (snapping after a jump such as an area change).
        let first = cam.focus.is_none();
        let jump = cam.focus.is_some_and(|f| f.distance(ppos) > 4.0);
        if first || jump {
            cam.focus = Some(ppos);
            cam.view_yaw = cam.yaw;
        }
        let yaw_diff = (cam.yaw - cam.view_yaw + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
        cam.view_yaw += yaw_diff * (1.0 - (-CAM_VIEW_RATE * dt).exp());
        let focus = cam.focus.unwrap_or(ppos);
        cam.focus = Some(focus + (ppos - focus) * (1.0 - (-CAM_FOCUS_RATE * dt).exp()));
        let ppos = cam.focus.unwrap_or(ppos);
        let notches = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y,
            MouseScrollUnit::Pixel => scroll.delta.y / 40.0,
        };
        cam.dist = (cam.dist - notches * 0.5).clamp(CAM_MIN_DIST, 12.0);

        let target = ppos + Vec3::Z * EYE;
        let yaw = cam.view_yaw;
        let look_at = |pitch: f32| Vec3::new(yaw.cos() * pitch.cos(), yaw.sin() * pitch.cos(), -pitch.sin());
        // Distance left clear along a view direction, against the rendered
        // room geometry (what the player sees). The walkmesh LOS test is not
        // used: its non-walk faces (bed and tank footprints) block views the
        // render geometry leaves open, which collapsed the camera into the
        // head at the 101per start.
        // Two rays per candidate: from the eye point along the view, and from
        // the chest to the same camera spot, so a console between camera and
        // body (101per start) counts as blocking even when the eye ray clears.
        let chest = ppos + Vec3::Z * 0.9;
        let mut clear = |look: Vec3, dist: f32| -> f32 {
            let eye_to = target - look * dist;
            let mut c = dist;
            for (o, len) in [(target, dist), (chest, chest.distance(eye_to))] {
                let from = coords::to_bevy(o);
                let Ok(d) = Dir3::new(coords::to_bevy(eye_to) - from) else { continue };
                if let Some((_, hit)) = ray.cast_ray(Ray3d::new(from, d), &settings).first()
                    && hit.distance < len + 0.3
                {
                    // Scale a chest-ray hit back onto the view ray.
                    c = c.min((hit.distance / len.max(0.01) * dist - 0.3).max(0.0));
                }
            }
            c
        };
        // Lift first: the smallest extra pitch that keeps the preferred
        // minimum distance clear, instead of zooming into the head. In tight
        // spots with no such pitch, take the lift with the most room.
        let (mut want_lift, mut allowed) = (0.0f32, 0.0f32);
        for k in 0..=6 {
            let lift = CAM_MAX_LIFT * k as f32 / 6.0;
            let c = clear(look_at((cam.pitch + lift).min(1.35)), cam.dist);
            if c > allowed + 0.05 {
                (want_lift, allowed) = (lift, c);
            }
            if c >= CAM_MIN_DIST {
                (want_lift, allowed) = (lift, c);
                break;
            }
        }
        let allowed = allowed.max(0.35);
        if std::env::var_os("K2_CAMDBG").is_some() {
            let cands: Vec<String> = (0..=6)
                .map(|k| {
                    let lift = CAM_MAX_LIFT * k as f32 / 6.0;
                    format!("{:.2}", clear(look_at((cam.pitch + lift).min(1.35)), cam.dist))
                })
                .collect();
            info!("camdbg p={ppos:.2} yaw={:.2} pitch={:.2} dist={:.1} lift->{want_lift:.2} allowed={allowed:.2} cands={cands:?}", cam.yaw, cam.pitch, cam.dist);
        }
        let ease = 1.0 - (-6.0 * dt).exp();
        cam.lift += (want_lift - cam.lift) * ease;
        cam.cur = if allowed < cam.cur { allowed } else { cam.cur + (allowed - cam.cur) * ease };
        let look = look_at((cam.pitch + cam.lift).min(1.35));
        let eye = target - look * cam.cur;
        // Hide the leader when the camera ends up inside the head.
        for mut v in &mut player_vis {
            let want = if cam.cur < 0.7 { Visibility::Hidden } else { Visibility::Inherited };
            if *v != want {
                *v = want;
            }
        }
        *t = Transform::from_translation(coords::to_bevy(eye)).looking_at(coords::to_bevy(target), Vec3::Y);
    }
}

/// Points the follow camera along the leader's facing (after an area load).
pub fn reset_camera_behind(cams: &mut Query<&mut PlayCam>, facing: f32) {
    let turn = cam_view_hook().map_or(0.0, |v| v.1);
    for mut c in cams.iter_mut() {
        c.yaw = facing + turn;
        c.view_yaw = facing + turn;
        c.focus = None;
        c.cur = c.dist;
    }
}

/// With `--autowalk`, log the player position and ground material once a
/// second so runs can be checked from the console.
fn autowalk_log(
    time: Res<Time>,
    args: Res<Args>,
    world: Res<WorldRes>,
    player: Query<(&Player, &PlayerMoveState)>,
    mut next: Local<f32>,
    mut elapsed: Local<f32>,
) {
    if args.autowalk.is_none() {
        return;
    }
    *elapsed += time.delta_secs();
    if *elapsed < *next {
        return;
    }
    *next += 1.0;
    let Ok((p, st)) = player.single() else { return };
    let w = &world.0.walk;
    let mat = w.ground(p.pos.x, p.pos.y, p.pos.z).map_or("NONE".to_owned(), |h| w.materials().name(h.material).to_owned());
    info!(
        "autowalk t={:.1}s pos=({:.2}, {:.2}, {:.2}) facing={:.0}deg speed={:.2} moving={} ground={mat}",
        *elapsed,
        p.pos.x,
        p.pos.y,
        p.pos.z,
        p.facing.to_degrees(),
        st.speed,
        st.moving
    );
}

#[cfg(test)]
mod tests;

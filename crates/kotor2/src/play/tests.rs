use std::io::Write;
use super::*;
use crate::game::{FrameEvents, MovieCards, NeedAreaLoad, PendingBarks, PendingVerb, Transition, click_object, world_tick};
use crate::presentation::Gate;
use bevy::time::{Real, TimePlugin, Virtual};
use k2_world::control::{Command, Control, Env, MoveToArgs, ObjectArgs, Reply};

#[test]
fn a_walk_without_progress_stalls_after_one_and_a_half_seconds() {
    let mut s = WalkStall::default();
    assert!(!s.advance(true, 0.0, 0.5));
    assert!(!s.advance(true, 0.0, 0.5));
    assert!(s.advance(true, 0.0, 0.5));
    // The clock restarts once it has fired.
    assert!(!s.advance(true, 0.0, 0.5));
}

#[test]
fn progress_or_a_frame_without_a_walk_resets_the_clock() {
    let mut s = WalkStall::default();
    assert!(!s.advance(true, 0.0, 1.0));
    assert!(!s.advance(true, 0.02, 1.0));
    assert!(!s.advance(true, 0.0, 1.0));
    assert!(!s.advance(false, 0.0, 1.0));
    assert!(!s.advance(true, 0.0, 1.0));
}

/// A world for the install, not yet entered into a module, or None (SKIP) without an install.
fn install_world() -> Option<k2_world::World> {
    let dir = std::env::var("K2_INSTALL").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP click-walk tests: no install (set K2_INSTALL)");
        return None;
    };
    Some(k2_world::World::new(install).unwrap_or_else(|e| panic!("the install does not load into a world: {e:?}")))
}

/// The world with 101per entered, or None (SKIP) without an install.
fn world() -> Option<k2_world::World> {
    let mut w = install_world()?;
    w.enter_module("101per", None).unwrap_or_else(|e| panic!("101per: enter_module: {e:?}"));
    Some(w)
}

/// A useable placeable and a walkable spot 8 m from it, from which the
/// placeable has a spot to walk to. The leader is put on that spot, so the
/// click is out of use reach.
fn click_target(w: &mut k2_world::World) -> Option<(ObjectId, Vec3)> {
    let leader = w.party_leader();
    let placeables: Vec<(ObjectId, Vec3)> = w.iter().filter(|o| o.placeable().is_some_and(|p| p.useable)).map(|o| (o.id, o.position)).take(60).collect();
    for (id, tp) in placeables {
        for k in 0..16 {
            let a = k as f32 * std::f32::consts::TAU / 16.0;
            let q = tp + Vec3::new(a.cos(), a.sin(), 0.0) * 8.0;
            let Some(h) = w.walk.ground(q.x, q.y, q.z + 1.0) else { continue };
            let from = Vec3::new(q.x, q.y, h.z);
            if w.use_point(from, id).is_some() {
                w.place(leader, from);
                return Some((id, from));
            }
        }
    }
    None
}

/// The game lines "Unable to reach target!" since the last call.
fn unreachable_lines(w: &mut k2_world::World) -> usize {
    w.drain_events().into_iter().filter(|e| matches!(e, k2_world::WorldEvent::Feedback { text, .. } if text == "Unable to reach target!")).count()
}

#[test]
fn a_walk_with_no_spot_ends_once_with_the_game_line() {
    let Some(mut w) = world() else { return };
    let Some((id, _)) = click_target(&mut w) else { panic!("no placeable out of reach in 101per") };
    let mut pending = PendingUse(Some(id));
    // Far off the walkmesh there is no start node, so no spot search begins.
    let far = Vec3::new(1.0e6, 0.0, 0.0);
    let mut walk = ClickWalk { target: Some(id), search: None, stand: None, stall: WalkStall::default() };
    let _ = w.drain_events();
    assert!(walk.heading(&mut w, &mut pending, far).is_none());
    assert!(pending.0.is_none());
    // A walking frame with a long step (the stall clock fires) reports nothing more.
    walk.progress(&mut w, &mut pending, far, far, true, 2.0);
    assert_eq!(unreachable_lines(&mut w), 1);
}

#[test]
fn a_stalled_walk_ends_once_after_one_and_a_half_seconds() {
    let Some(mut w) = world() else { return };
    let Some((id, from)) = click_target(&mut w) else { panic!("no placeable out of reach in 101per") };
    let mut pending = PendingUse(Some(id));
    let mut walk = ClickWalk::default();
    let _ = w.drain_events();
    walk.begin(&mut w);
    // The leader stays put, 0.5 s a frame, as move_player runs it: frames
    // spent searching for a spot are not walking.
    let mut walking_frames = 0;
    for _ in 0..40 {
        if pending.0.is_none() {
            break;
        }
        let walking = walk.heading(&mut w, &mut pending, from).is_some();
        walking_frames += usize::from(walking);
        walk.progress(&mut w, &mut pending, from, from, walking, 0.5);
    }
    assert!(pending.0.is_none(), "the stall ends the walk");
    assert_eq!(walking_frames, 3, "it ends on the third 0.5 s walking frame without progress");
    assert_eq!(unreachable_lines(&mut w), 1);
}

#[test]
fn a_new_click_starts_the_walk_afresh() {
    let Some(mut w) = world() else { return };
    let Some((id, from)) = click_target(&mut w) else { panic!("no placeable out of reach in 101per") };
    // An earlier walk's state: a stand and a stall clock.
    let mut walk = ClickWalk { target: Some(id), search: None, stand: Some(Vec3::ZERO), stall: WalkStall(1.2) };
    let mut pending = PendingUse(None);
    let (mut verb, mut barks) = (PendingVerb::default(), PendingBarks::default());
    let _ = w.drain_events();
    click_object(&mut w, &mut walk, &mut pending, &mut verb, &mut barks, id, from);
    assert_eq!(pending.0, Some(id), "the click starts a walk to the object");
    assert!(walk.target.is_none() && walk.search.is_none() && walk.stand.is_none());
    assert_eq!(walk.stall.0, 0.0);
    // The next frame searches for a spot afresh: nothing stale is reused.
    assert!(walk.heading(&mut w, &mut pending, from).is_none(), "the first frame is spent searching");
    assert_eq!(walk.target, Some(id));
    assert!(walk.search.is_some() || walk.stand.is_some(), "the first frame begins a fresh search");
}

#[test]
fn an_in_reach_click_ends_an_earlier_walk() {
    let Some(mut w) = world() else { return };
    let Some((id, _)) = click_target(&mut w) else { panic!("no placeable out of reach in 101per") };
    let other = w.iter().find(|o| o.id != id && o.placeable().is_some_and(|p| p.useable)).map(|o| o.id).expect("a second useable placeable");
    // The leader stands beside the placeable, so the click is in reach.
    let leader = w.party_leader();
    let at = w.get(id).expect("object").position + Vec3::new(0.5, 0.0, 0.0);
    w.place(leader, at);
    let from = w.get(leader).expect("leader").position;
    assert!(w.in_use_reach(from, id), "the click is in reach");
    let mut walk = ClickWalk { target: Some(other), search: None, stand: Some(Vec3::ZERO), stall: WalkStall(1.0) };
    let mut pending = PendingUse(Some(other));
    let (mut verb, mut barks) = (PendingVerb::default(), PendingBarks::default());
    click_object(&mut w, &mut walk, &mut pending, &mut verb, &mut barks, id, from);
    assert!(pending.0.is_none(), "the earlier walk to {other} is dropped, not resumed");
    assert!(walk.target.is_none() && walk.stand.is_none() && walk.stall.0 == 0.0);
}

#[test]
fn a_route_search_that_runs_on_is_not_walking() {
    let Some(mut w) = world() else { return };
    let Some((id, from)) = click_target(&mut w) else { panic!("no placeable out of reach in 101per") };
    let (lo, hi) = w.walk.bounds();
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut unit = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed >> 40) as f32 / (1u64 << 24) as f32
    };
    // Four nodes a frame, so a route a few metres long takes several frames.
    w.route_step = 4;
    // Stands on the floor of 101per until one's route search runs past its first frame.
    let mut runs_on = 0;
    for _ in 0..400 {
        let x = lo.x + unit() * (hi.x - lo.x);
        let y = lo.y + unit() * (hi.y - lo.y);
        let Some(h) = w.walk.ground(x, y, hi.z + 10.0) else { continue };
        let stand = Vec3::new(x, y, h.z);
        w.forget_leader_path();
        let mut walk = ClickWalk { target: Some(id), search: None, stand: Some(stand), stall: WalkStall::default() };
        let mut pending = PendingUse(Some(id));
        loop {
            let heading = walk.heading(&mut w, &mut pending, from);
            if !w.leader_route_pending() {
                break;
            }
            runs_on += 1;
            assert!(heading.is_none(), "a frame whose route search runs on is not walking");
        }
        if runs_on > 0 {
            break;
        }
    }
    assert!(runs_on > 0, "no stand in 101per needed more than one frame's route search");
}

/// Clicks open the PeragusDoor1 doors the leader can reach from the wake-up spot, nearest reachable first.
/// Each click runs the way the game runs it: a heading each frame, the leader moved
/// by player_move, progress after the step, and the use once the leader stands in
/// reach (the game's arrival check, move_player's arrival in game.rs).
#[test]
fn clicks_open_every_peragus_door_from_the_wake_up_spot() {
    let Some(mut w) = world() else { return };
    // The intro runs first, as door_talk's settled world does; then the leader stands at the wake-up waypoint.
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let _ = w.drain_events();
    let pc = w.party_leader();
    let wake = w.by_tag("WP_wakeup_donnie", 0).expect("101per has WP_wakeup_donnie");
    let wake_at = w.get(wake).expect("the waypoint is in the world").position;
    w.place(pc, wake_at);
    let placed = w.get(pc).expect("the leader is in the world").position;
    assert!(placed.truncate().distance(wake_at.truncate()) < 0.5, "the leader is at {placed:?}, not the wake-up waypoint {wake_at:?}");
    let dt = 1.0 / 30.0;
    let speed = w.get(pc).and_then(|o| o.creature()).map_or(1.75, |c| c.walk_speed);
    let mut clicked: Vec<ObjectId> = Vec::new();
    loop {
        let from = w.get(pc).expect("the leader is in the world").position;
        // The nearest closed door whose use stand the leader can reach now: a door behind a shut one waits its turn.
        let mut closed: Vec<(ObjectId, f32)> = w
            .iter()
            .filter(|o| o.kind == k2_world::ObjectKind::Door && o.tag == "PeragusDoor1" && !o.door().is_some_and(|d| d.open))
            .map(|o| (o.id, o.position.truncate().distance(from.truncate())))
            .collect();
        if closed.is_empty() {
            break;
        }
        closed.sort_by(|a, b| a.1.total_cmp(&b.1));
        // None reachable from here is the state of the game (the rest lie behind the medbay gate): the clicks stop.
        let Some(door) = closed.iter().map(|c| c.0).find(|&d| use_point_unlocked(&w, from, d).is_some()) else {
            break;
        };
        let (mut verb, mut barks) = (PendingVerb::default(), PendingBarks::default());
        let mut walk = ClickWalk::default();
        let mut pending = PendingUse(None);
        let _ = w.drain_events();
        click_object(&mut w, &mut walk, &mut pending, &mut verb, &mut barks, door, from);
        assert_eq!(pending.0, Some(door), "the click starts a walk to PeragusDoor1 {door}");
        let mut used = false;
        for _ in 0..3000 {
            let pos = w.get(pc).expect("the leader is in the world").position;
            if pending.0.is_some() && w.in_use_reach(pos, door) {
                // The arrival check of the game: the pending use runs.
                w.forget_leader_path();
                w.use_object(pc, door);
                used = true;
                break;
            }
            if pending.0.is_none() {
                break;
            }
            match walk.heading(&mut w, &mut pending, pos) {
                Some(dir) => {
                    let to = w.player_move(pos, dir * speed * dt, w.leader_clearance());
                    w.place(pc, to);
                    let to = w.get(pc).expect("the leader is in the world").position;
                    walk.progress(&mut w, &mut pending, pos, to, true, dt);
                }
                None => walk.progress(&mut w, &mut pending, pos, pos, false, dt),
            }
        }
        let at = w.get(pc).expect("the leader is in the world").position;
        assert!(used, "the click walk to PeragusDoor1 {door} did not reach it: leader at {at:?}");
        for _ in 0..120 {
            w.tick(dt);
        }
        assert!(w.get(door).and_then(|o| o.door()).is_some_and(|d| d.open), "PeragusDoor1 {door} did not open from the click");
        clicked.push(door);
    }
    assert!(!clicked.is_empty(), "no PeragusDoor1 is reachable by a click from the wake-up spot");
    // The clicks stop only when no door still shut has a use stand the leader can reach
    // without crossing a locked door: every door still shut is out of that reach at the end.
    let at = w.get(pc).expect("the leader is in the world").position;
    let shut: Vec<ObjectId> = w
        .iter()
        .filter(|o| o.kind == k2_world::ObjectKind::Door && o.tag == "PeragusDoor1" && !o.door().is_some_and(|d| d.open))
        .map(|o| o.id)
        .collect();
    for door in shut {
        assert!(
            use_point_unlocked(&w, at, door).is_none(),
            "PeragusDoor1 {door} is still shut and has a use stand from {at:?} that crosses no locked door: the clicks stopped with it reachable"
        );
    }
}

/// The use stand for `target` from `from`, when the route there runs through no locked
/// door: a walk that stops at a locked door does not reach the stand.
fn use_point_unlocked(w: &k2_world::World, from: bevy::prelude::Vec3, target: ObjectId) -> Option<bevy::prelude::Vec3> {
    let stand = w.use_point(from, target)?;
    let mut legs = vec![from];
    legs.extend(w.walk.find_path(from, stand, w.leader_clearance()).unwrap_or_default());
    let locked = legs.windows(2).filter_map(|leg| w.door_across(leg[0], leg[1])).any(|d| w.get(d).and_then(|o| o.door()).is_some_and(|d| d.locked));
    (!locked).then_some(stand)
}

/// The world after the intro, as door_talk's settled world is: 101per with its talks run to their end.
fn intro_world() -> Option<k2_world::World> {
    let mut w = world()?;
    w.auto_dialog = true;
    for _ in 0..300 {
        w.tick(0.1);
    }
    w.auto_dialog = false;
    w.end_conversation(true);
    let _ = w.drain_events();
    Some(w)
}

/// Runs `f` on the game's world resource.
fn with_world<R>(app: &mut App, f: impl FnOnce(&mut k2_world::World) -> R) -> R {
    f(&mut app.world_mut().resource_mut::<WorldRes>().0)
}

/// Regression (live): after the intro the leader stands at the bed spot, and a walk 2 m south
/// of it stands still. The control API walks the world's leader, but move_player kept its own
/// copy of the position while the player controls the leader, and wrote that copy back over
/// the world every frame, so each step of the walk was undone.
#[test]
fn a_control_walk_moves_the_leader_while_the_player_has_control() {
    let Some(w) = intro_world() else { return };
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.build().disable::<TimePlugin>())
        .init_resource::<Time>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Paused>()
        .init_resource::<InConversation>()
        .init_resource::<PendingUse>()
        .init_resource::<ClickWalk>()
        .init_resource::<Args>()
        .init_resource::<MovieCards>()
        .insert_resource(CamMode::Play)
        .insert_resource(WorldRes(w))
        .add_systems(Update, move_player);

    // The save position after the intro (the bed spot), and a point 2 m south of it on open floor.
    let spot = Vec3::new(-1.29, 19.93, 9.02);
    let to = with_world(&mut app, |w| {
        let pc = w.party_leader();
        w.place(pc, spot);
        let ground = w.walk.ground_nearest(spot.x, spot.y - 2.0, spot.z)?;
        Some(Vec3::new(spot.x, spot.y - 2.0, ground.z))
    })
    .expect("the bed spot has open floor 2 m south of it");
    // The Bevy side holds the leader as the game does once the world has given it back control.
    let facing = with_world(&mut app, |w| w.get(w.party_leader()).map_or(0.0, |o| o.bearing)) + std::f32::consts::FRAC_PI_2;
    app.world_mut().spawn((Player { pos: spot, facing }, PlayerMoveState::default(), Transform::from_translation(spot)));

    let root = std::env::temp_dir();
    let env = k2_world::control::Env { save_root: root.as_path(), paused: false, movie: false };
    let mut control = k2_world::control::Control::default();
    let cmd = k2_world::control::Command::MoveTo(k2_world::control::MoveToArgs { point: Some(to.to_array()), ..Default::default() });
    let dt = 1.0 / 30.0;
    let mut reply = with_world(&mut app, |w| control.submit(w, &env, cmd));
    for _ in 0..(60.0 / dt) as usize {
        if reply.is_some() {
            break;
        }
        app.world_mut().resource_mut::<Time>().advance_by(std::time::Duration::from_secs_f32(dt));
        app.update();
        reply = with_world(&mut app, |w| {
            w.tick(dt);
            control.poll(w, &env)
        });
    }
    let at = with_world(&mut app, |w| w.get(w.party_leader()).map_or(spot, |o| o.position));
    let reply = reply.expect("the walk ends within a minute of game time");
    assert!(reply.ok, "the walk 2 m south of the bed spot did not arrive: {}; the leader stands at {at:?}", reply.message);
    assert!(at.truncate().distance(to.truncate()) <= 0.6, "the walk ended at {at:?}, not at {to:?}");
}

const UNREACHABLE: &str = "Unable to reach target!";
const FRAME: f32 = 1.0 / 30.0;

/// The game's app for the systems that move the leader and tick the world: move_player
/// (Input), then world_tick (Tick), as game.rs chains them. The Player component is made as
/// sync_objects makes it when the area loads.
fn game_app(w: k2_world::World) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins.build().disable::<TimePlugin>())
        .init_resource::<Time>()
        .init_resource::<Time<Real>>()
        .init_resource::<Time<Virtual>>()
        .init_resource::<ButtonInput<KeyCode>>()
        .init_resource::<Paused>()
        .init_resource::<InConversation>()
        .init_resource::<PendingUse>()
        .init_resource::<ClickWalk>()
        .init_resource::<Args>()
        .init_resource::<FrameEvents>()
        .init_resource::<MovieCards>()
        .init_resource::<Transition>()
        .init_resource::<Gate>()
        .insert_resource(CamMode::Play)
        .insert_resource(WorldRes(w))
        .add_systems(Update, (move_player, world_tick).chain());
    let (pos, facing) = with_world(&mut app, |w| {
        let o = w.get(w.party_leader()).expect("the leader is in the world");
        (o.position, o.bearing + std::f32::consts::FRAC_PI_2)
    });
    app.world_mut().spawn((Player { pos, facing }, PlayerMoveState { running: true, ..default() }, Transform::from_translation(pos)));
    app
}

/// Puts the Bevy copy of the leader (Player) at `pos`, facing `facing`.
fn set_bevy_leader(app: &mut App, pos: Vec3, facing: f32) {
    let world = app.world_mut();
    let mut q = world.query::<&mut Player>();
    for mut p in q.iter_mut(world) {
        p.pos = pos;
        p.facing = facing;
    }
}

/// The scripted branch of sync_objects: while the world has the leader (a script, a dialog),
/// the Bevy copy follows the world.
fn mirror_scripted_leader(app: &mut App) {
    let scripted = with_world(app, |w| {
        if w.player_controlled() && w.conversation.is_none() {
            return None;
        }
        w.get(w.party_leader()).map(|o| (o.position, o.bearing + std::f32::consts::FRAC_PI_2))
    });
    if let Some((pos, facing)) = scripted {
        set_bevy_leader(app, pos, facing);
    }
}

/// One frame of the game's systems: the clocks move on, movie cards are dismissed (the test has
/// no movie playback), the systems run, then the scripted branch of sync_objects.
fn game_frame(app: &mut App, dt: f32) {
    let d = std::time::Duration::from_secs_f32(dt);
    app.world_mut().resource_mut::<Time>().advance_by(d);
    app.world_mut().resource_mut::<Time<Real>>().advance_by(d);
    app.world_mut().resource_mut::<Time<Virtual>>().advance_by(d);
    app.world_mut().resource_mut::<MovieCards>().queue.clear();
    app.update();
    mirror_scripted_leader(app);
}

/// Runs a control command to its end a frame at a time, as the game's remote serves it.
fn run_control(app: &mut App, control: &mut Control, env: &Env, cmd: Command) -> Reply {
    if let Some(reply) = with_world(app, |w| control.submit(w, env, cmd)) {
        return reply;
    }
    for _ in 0..(120.0 / FRAME) as usize {
        game_frame(app, FRAME);
        if let Some(reply) = with_world(app, |w| control.poll(w, env)) {
            return reply;
        }
    }
    panic!("the control command did not end within 120 s of game time");
}

/// A point `radius` metres from `from` on walkable floor that a walk from `from` ends at.
fn open_floor_point(w: &k2_world::World, from: Vec3, radius: f32) -> Option<Vec3> {
    (0..16).find_map(|k| {
        let a = k as f32 * std::f32::consts::TAU / 16.0;
        let q = from.truncate() + bevy::prelude::Vec2::new(a.cos(), a.sin()) * radius;
        let to = Vec3::new(q.x, q.y, w.walk.ground(q.x, q.y, from.z + 1.0)?.z);
        let end = *w.walk.find_path(from, to, w.leader_clearance())?.last()?;
        (end.truncate().distance(to.truncate()) <= 0.6).then_some(to)
    })
}

/// The game's app on 101per after the intro, with the leader under the player's control at
/// (-12.30, 0.58), where a walk by keyboard left it (the Bevy copy holds that too). Returns the
/// app, the start and the leader's facing, or None (SKIP) without an install.
fn walking_game() -> Option<(App, Vec3, f32)> {
    let mut w = install_world()?;
    let mut need = NeedAreaLoad::default();
    assert!(crate::game::enter_module(&mut w, &mut need, "101per", None), "101per enters as the game enters it");
    let mut app = game_app(w);

    // The intro runs with its talks played through, then the leader has the player's control.
    with_world(&mut app, |w| w.auto_dialog = true);
    for _ in 0..300 {
        game_frame(&mut app, 0.1);
    }
    with_world(&mut app, |w| {
        w.auto_dialog = false;
        w.end_conversation(true);
        let _ = w.drain_events();
    });
    for _ in 0..600 {
        if with_world(&mut app, |w| w.player_controlled()) {
            break;
        }
        game_frame(&mut app, 0.1);
    }
    assert!(with_world(&mut app, |w| w.player_controlled()), "the leader is under the player's control after the intro");

    let (start, facing) = with_world(&mut app, |w| {
        let pc = w.party_leader();
        let z = w.walk.ground_nearest(-12.30, 0.58, 9.09).expect("(-12.30, 0.58) is on walkable floor").z;
        let p = Vec3::new(-12.30, 0.58, z);
        w.place(pc, p);
        (p, w.get(pc).map_or(0.0, |o| o.bearing) + std::f32::consts::FRAC_PI_2)
    });
    set_bevy_leader(&mut app, start, facing);
    Some((app, start, facing))
}

/// Gate for the live walk failures: the game's systems on 101per, entered as game.rs enters a
/// module. The leader walks 2 m in open floor on a control MoveTo, then uses the Medical
/// Computer from where it stood. The harness-only tests passed while these walks failed live.
#[test]
fn the_game_walks_the_leader_for_a_control_move_and_a_use() {
    let Some((mut app, start, facing)) = walking_game() else { return };

    let env_root = std::env::temp_dir();
    let env = Env { save_root: env_root.as_path(), paused: false, movie: false };
    let mut control = Control::default();

    // A 2 m move in open floor.
    let to = with_world(&mut app, |w| open_floor_point(w, start, 2.0)).expect("there is open floor 2 m from the start");
    let reply = run_control(&mut app, &mut control, &env, Command::MoveTo(MoveToArgs { point: Some(to.to_array()), ..Default::default() }));
    let at = with_world(&mut app, |w| w.get(w.party_leader()).map_or(start, |o| o.position));
    assert!(reply.ok, "the 2 m move from {start:?} to {to:?} answered: {}; the leader stands at {at:?}", reply.message);
    assert!(at.truncate().distance(to.truncate()) <= 0.6, "the move ended at {at:?}, not at {to:?}");

    // A use from the start spot: the walk goes into the Medical Computer's reach.
    with_world(&mut app, |w| {
        let pc = w.party_leader();
        w.place(pc, start);
    });
    set_bevy_leader(&mut app, start, facing);
    let computer = with_world(&mut app, |w| {
        w.iter().find(|o| o.kind == k2_world::ObjectKind::Placeable && w.name_of(o.id) == "Medical Computer").map(|o| o.id)
    })
    .expect("101per has the Medical Computer");
    let reply = run_control(&mut app, &mut control, &env, Command::Interact(ObjectArgs { object: computer }));
    let at = with_world(&mut app, |w| w.get(w.party_leader()).map_or(start, |o| o.position));
    assert_ne!(reply.message, UNREACHABLE, "the Medical Computer from {start:?}: {}; the leader stands at {at:?}", reply.message);
    assert!(with_world(&mut app, |w| w.in_use_reach(at, computer)), "the leader at {at:?} is out of reach of the Medical Computer");
}

/// The leader's Bevy-side state: its movement state, position and facing.
fn bevy_leader(app: &mut App) -> (PlayerMoveState, Vec3, f32) {
    let world = app.world_mut();
    let mut q = world.query::<(&Player, &PlayerMoveState)>();
    let (p, st) = q.iter(world).next().expect("the leader has a Player");
    (*st, p.pos, p.facing)
}

/// Regression (live): a control walk slid the leader, its movement animation did not play. While
/// the world walks the leader, the Bevy side has it moving at walk speed and facing along its
/// route; once it has arrived it is idle.
#[test]
fn a_control_walk_animates_and_turns_the_leader() {
    let Some((mut app, start, _)) = walking_game() else { return };
    let to = with_world(&mut app, |w| open_floor_point(w, start, 4.0)).expect("there is open floor 4 m from the start");
    let env_root = std::env::temp_dir();
    let env = Env { save_root: env_root.as_path(), paused: false, movie: false };
    let mut control = Control::default();
    let cmd = Command::MoveTo(MoveToArgs { point: Some(to.to_array()), ..Default::default() });
    let mut reply = with_world(&mut app, |w| control.submit(w, &env, cmd));
    let mut last = start;
    let mut walking = 0;
    for _ in 0..(60.0 / FRAME) as usize {
        if reply.is_some() {
            break;
        }
        game_frame(&mut app, FRAME);
        let (st, pos, facing) = bevy_leader(&mut app);
        let step = (pos - last).truncate();
        if step.length() > 0.02 {
            walking += 1;
            assert!(st.moving && st.speed > 0.2, "the leader walks from {last:?} to {pos:?} but its movement state is {st:?}");
            assert_eq!(st.world_driven, Some(false), "a control walk at walk speed runs its animation as a walk: {st:?}");
            let heading = step.y.atan2(step.x);
            let turn = (facing - heading + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU) - std::f32::consts::PI;
            assert!(turn.abs() < 0.5, "the leader faces {facing:.2} while it walks along {heading:.2} at {pos:?}");
        }
        last = pos;
        reply = with_world(&mut app, |w| control.poll(w, &env));
    }
    let reply = reply.expect("the walk ends within a minute of game time");
    assert!(reply.ok, "the walk to {to:?} from {start:?} answered: {}", reply.message);
    assert!(walking >= 10, "the leader walked on only {walking} frames");
    for _ in 0..3 {
        game_frame(&mut app, FRAME);
    }
    let (st, _, _) = bevy_leader(&mut app);
    assert!(!st.moving, "the leader still animates as moving after it arrived: {st:?}");
}

/// Starts a conversation with a creature in 101per, so a cutscene holds the party.
fn start_any_talk(w: &mut k2_world::World) {
    let pc = w.party_leader();
    let talks: Vec<(ObjectId, String)> = w.iter().filter(|o| o.kind == k2_world::ObjectKind::Creature && o.id != pc).filter_map(|o| o.conversation.clone().map(|d| (o.id, d))).collect();
    for (npc, dlg) in talks {
        if w.start_conversation(npc, pc, &dlg) && w.conversation.is_some() {
            return;
        }
    }
    panic!("101per has a conversation to start");
}

/// Regression (live): a cutscene holds the party, so the keyboard does not move the leader while a
/// conversation runs. The keys walk the leader before the conversation, which checks that they work.
#[test]
fn keyboard_does_not_move_the_leader_during_a_cutscene() {
    let Some((mut app, start, _)) = walking_game() else { return };
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().press(KeyCode::KeyW);
    for _ in 0..30 {
        game_frame(&mut app, FRAME);
    }
    let walked = with_world(&mut app, |w| w.get(w.party_leader()).map_or(start, |o| o.position));
    assert!(walked.truncate().distance(start.truncate()) > 0.3, "the keys walk the leader before a cutscene: {walked:?}");

    with_world(&mut app, start_any_talk);
    let held = with_world(&mut app, |w| w.get(w.party_leader()).map_or(start, |o| o.position));
    for _ in 0..60 {
        game_frame(&mut app, FRAME);
    }
    let after = with_world(&mut app, |w| w.get(w.party_leader()).map_or(start, |o| o.position));
    app.world_mut().resource_mut::<ButtonInput<KeyCode>>().release(KeyCode::KeyW);
    assert_eq!(after, held, "the keyboard moved the leader during the cutscene: from {held:?} to {after:?}");
}

/// The camera follows a walking leader: its yaw swings round to the leader's facing within about half a
/// second, without a snap, the short way round. Standing still, a manual turn, and walking backward leave
/// it where it was.
#[test]
fn the_camera_swings_behind_a_walking_leader() {
    let dt = 1.0 / 60.0;
    let (start, facing) = (2.0_f32, 0.0_f32);
    let mut yaw = start;
    let mut largest_step = 0.0_f32;
    for _ in 0..30 {
        let next = camera_swing(yaw, facing, true, false, false, dt);
        largest_step = largest_step.max((next - yaw).abs());
        yaw = next;
    }
    assert!(
        (yaw - facing).abs() < 0.06 * start,
        "the camera is still {:.2} rad from the leader's facing after 0.5 s of walking",
        (yaw - facing).abs()
    );
    assert!(largest_step < 0.25, "the camera snapped: a step of {largest_step:.2} rad in one frame");

    // The short way round: from just past pi to just short of -pi, the camera turns on through pi.
    let wrapped = camera_swing(3.0, -3.0, true, false, false, dt);
    assert!(wrapped > 3.0, "the camera turns the short way round: {wrapped:.3}");

    assert_eq!(camera_swing(start, facing, false, false, false, dt), start, "standing still leaves the camera");
    assert_eq!(camera_swing(start, facing, true, true, false, dt), start, "a manual turn leaves the camera");
    assert_eq!(camera_swing(start, facing, true, false, true, dt), start, "walking backward leaves the camera");
}

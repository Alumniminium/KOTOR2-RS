use super::*;

/// A fresh folder under the temp directory for one test.
fn scratch(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("kotor2-remote-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn files_are_confined_to_their_folder() {
    let dir = scratch("confine");
    let root = dir.join("shots");
    std::fs::create_dir_all(root.join("sub")).unwrap();
    let inside = |p: &str| confine_in(&root, Output::Shot, Some(p));

    let named = inside("a.png").unwrap();
    assert_eq!(named.parent(), root.canonicalize().ok().as_deref());
    assert!(inside("sub/b.png").is_ok(), "a folder under the root is fine");
    assert!(confine_in(&root, Output::Shot, None).unwrap().starts_with(root.canonicalize().unwrap()), "the default is under the root");

    assert!(inside("../escape.png").is_err(), "a parent folder is refused");
    assert!(inside("sub/../../escape.png").is_err(), "a path that climbs out is refused");
    assert!(inside("/etc/k2-escape.png").is_err(), "an absolute path elsewhere is refused");
    assert!(inside("..").is_err() && inside("sub/..").is_err(), "a folder is not a file");
    assert!(inside("missing/x.png").is_err(), "the folder must exist");

    #[cfg(unix)]
    {
        let outside = dir.join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        std::os::unix::fs::symlink(&outside, root.join("link")).unwrap();
        assert!(inside("link/x.png").is_err(), "a symlink out of the root is refused");
        assert!(!outside.join("x.png").exists());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_existing_file_is_never_taken_over() {
    let dir = scratch("exists");
    let root = dir.join("traces");
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("keep.json"), b"a save of the trace").unwrap();
    assert!(confine_in(&root, Output::Trace, Some("keep.json")).is_err());
    assert_eq!(std::fs::read(root.join("keep.json")).unwrap(), b"a save of the trace");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn trace_start_refuses_a_path_outside_its_folder() {
    let outside = std::env::temp_dir().join(format!("k2-remote-never-written-{}.json", std::process::id()));
    let reply = trace_start(Some(outside.to_str().unwrap()), None);
    assert!(!reply.ok, "{}", reply.message);
    assert!(!outside.exists(), "nothing was created outside the trace folder");
}

#[test]
fn points_must_be_numbers_inside_the_window() {
    let size = Some(Vec2::new(800.0, 600.0));
    assert!(remote_input::check_point([0.0, 0.0], size).is_ok());
    assert!(remote_input::check_point([799.5, 599.5], size).is_ok());
    assert!(remote_input::check_point([800.0, 10.0], size).is_err(), "the right edge is outside");
    assert!(remote_input::check_point([-1.0, 10.0], size).is_err());
    assert!(remote_input::check_point([f32::NAN, 10.0], size).is_err());
    assert!(remote_input::check_point([10.0, f32::INFINITY], size).is_err());
    assert!(remote_input::check_point([10.0, 10.0], None).is_err(), "no window, no pointer");
    assert!(check_points(&[[1.0, 1.0], [900.0, 1.0]], size).is_err(), "every point is checked");
}

/// A headless app with the window and input systems of the game's frame.
fn input_app() -> App {
    use bevy::input::InputPlugin;
    use bevy::window::CursorMoved;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .add_message::<CursorMoved>()
        .init_resource::<InputQueue>()
        .add_systems(Update, remote_input::deliver_input);
    app.world_mut().spawn((Window::default(), PrimaryWindow));
    app.update();
    app
}

#[test]
fn stop_releases_a_key_held_by_a_key_down_that_has_finished() {
    let mut app = input_app();
    let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
    app.world_mut().resource_mut::<InputQueue>().push(now, remote_input::plan_key_down("KeyW", &[]).unwrap());
    app.update();
    app.update();
    assert!(app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW), "the key_down holds the key");
    // The key_down's job has answered: no job is running.
    let mut job = None;
    let answered = {
        let mut queue = app.world_mut().resource_mut::<InputQueue>();
        stop_input(&mut queue, &mut job, now)
    };
    assert_eq!(answered, None);
    app.update();
    app.update();
    assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW), "stop released the held key");
}

#[test]
fn stop_cancels_a_running_input_job_and_answers_its_client() {
    let mut app = input_app();
    let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
    // A key goes down and is held (its key_down has delivered), and a move is still queued.
    app.world_mut().resource_mut::<InputQueue>().push(now, remote_input::plan_key_down("KeyW", &[]).unwrap());
    app.update();
    app.update();
    assert!(app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW), "the key_down holds the key");
    app.world_mut().resource_mut::<InputQueue>().push(now + 60.0, remote_input::plan_mouse_move(5.0, 5.0));
    let mut job = Some(Job { client: 7, work: Work::Input { reply: Reply::ok("typing"), deadline: now + 30.0 } });
    let answered = {
        let mut queue = app.world_mut().resource_mut::<InputQueue>();
        stop_input(&mut queue, &mut job, now)
    };
    assert_eq!(answered, Some(7), "the waiting client is answered");
    assert!(job.is_none(), "the job is gone");
    app.update();
    app.update();
    assert!(!app.world().resource::<ButtonInput<KeyCode>>().pressed(KeyCode::KeyW), "stop released the held key");
    assert!(app.world().resource::<InputQueue>().is_idle(), "the pending move was dropped, so only the release was left to deliver");
}

#[test]
fn the_phase_order_puts_the_movie_first() {
    assert_eq!(movie::phase(true, true, true, true, true, true), "movie");
    assert_eq!(movie::phase(false, false, true, false, false, false), "loading");
    assert_eq!(movie::phase(false, false, false, false, false, false), "menu");
    assert_eq!(movie::phase(true, false, false, true, false, true), "dialog");
    assert_eq!(movie::phase(true, false, false, false, true, true), "minigame");
    assert_eq!(movie::phase(true, false, false, false, false, true), "paused");
    assert_eq!(movie::phase(true, false, false, false, false, false), "game");
}

/// A held key for the rule tests: 0.5 s of game time from game time 10.0.
fn hold_half_second() -> KeyHold {
    KeyHold { key: "KeyW".into(), modifiers: Vec::new(), hold_s: 0.5, start: 10.0, until: 10.5, limit: 1000.0, from: [0.0; 3], released: None, focus: None }
}

#[test]
fn a_held_key_comes_up_when_the_game_clock_has_run_its_hold() {
    let hold = hold_half_second();
    // Wall-clock time passes, but the game clock stands still: the key stays down.
    assert_eq!(hold_step(&hold, 10.0, 3.0, None), HoldStep::Keep, "no game time has passed");
    assert_eq!(hold_step(&hold, 10.4, 4.0, None), HoldStep::Keep, "0.4 s of game time is not 0.5 s");
    // A frame over the clamp that carries the game clock past the hold releases it.
    let HoldStep::Release(why) = hold_step(&hold, 10.6, 4.1, None) else { panic!("the hold is over") };
    assert!(why.starts_with("held 0.50 s of game time"), "{why}");
}

#[test]
fn a_stopped_game_releases_a_held_key_at_once() {
    let hold = hold_half_second();
    let HoldStep::Release(why) = hold_step(&hold, 10.2, 2.0, Some("is paused")) else { panic!("a pause releases the key") };
    assert!(why.starts_with("released early: the game is paused after 0.20 s of game time"), "{why}");
    assert!(matches!(hold_step(&hold, 10.2, 2.0, Some("is in a conversation")), HoldStep::Release(_)));
}

#[test]
fn a_stalled_game_clock_releases_at_the_wall_clock_limit() {
    let mut hold = hold_half_second();
    hold.limit = 5.0;
    assert_eq!(hold_step(&hold, 10.1, 4.9, None), HoldStep::Keep, "still within the limit");
    let HoldStep::Release(why) = hold_step(&hold, 10.1, 5.1, None) else { panic!("past the limit the key comes up") };
    assert!(why.contains("wall-clock limit") && why.contains("0.10 of 0.50 s"), "{why}");
}

#[test]
fn the_game_stopped_reasons_are_named_in_the_order_they_are_checked() {
    assert_eq!(stopped_reason(false, false, false, false, false), Some("is not in the game"));
    assert_eq!(stopped_reason(true, false, false, false, true), Some("is showing a movie"));
    assert_eq!(stopped_reason(true, true, false, false, false), Some("is loading"));
    assert_eq!(stopped_reason(true, false, true, true, false), Some("is paused"));
    assert_eq!(stopped_reason(true, false, false, true, false), Some("is in a conversation"));
    assert_eq!(stopped_reason(true, false, false, false, false), None, "a game that runs holds keys");
}

#[test]
fn the_reply_reports_the_horizontal_distance_the_leader_moved() {
    assert!((moved_m([0.0, 0.0, 0.0], [3.0, 4.0, 9.0]) - 5.0).abs() < 1e-5, "height does not count");
    let hold = hold_half_second();
    let reply = hold_reply(&hold, "held 0.50 s of game time", [3.0, 4.0, 0.0], 0.5);
    assert!(reply.ok && reply.message.contains("moved 5.00 m"), "{}", reply.message);
    let data = reply.data.expect("the reply carries its numbers");
    assert_eq!(data["held_s"], 0.5);
    assert_eq!(data["moved_m"], 5.0);
}

#[test]
fn observe_names_the_open_window_in_its_message_and_its_data() {
    let line = "open: container window 'Corpse' (2 items)";
    let reply = with_open(Reply::ok("Test at 1.0 s"), line);
    assert_eq!(reply.message, format!("{line}; Test at 1.0 s"));
    assert_eq!(reply.data.expect("the data is kept").get("open").and_then(|v| v.as_str()), Some(line));
}

#[test]
fn raw_input_names_the_window_it_went_to() {
    let mut job = Some(Job { client: 1, work: Work::Input { reply: Reply::ok("key down: 1 input events delivered"), deadline: 1.0 } });
    attach_focus(&mut job, "container");
    let Some(Job { work: Work::Input { reply, .. }, .. }) = job else { panic!("the input job is still there") };
    assert_eq!(reply.message, "key down: 1 input events delivered, input went to the open container window");
}

#[test]
fn a_call_that_waited_says_how_long_it_waited() {
    let waited = Waited { text: "waited 3.2 s for Kreia to finish speaking".to_owned(), secs: 3.2 };
    let reply = with_note(Reply::ok("saved"), Some(&waited));
    assert_eq!(reply.message, "waited 3.2 s for Kreia to finish speaking; saved");
    assert_eq!(reply.data.expect("the data carries the seconds").get("waited_s").and_then(|v| v.as_f64()), Some(3.2));
    let unchanged = with_note(Reply::ok("saved"), None);
    assert_eq!(unchanged.message, "saved", "a call that did not wait is unchanged");
    assert!(unchanged.data.is_none());
}

#[test]
fn a_goal_or_job_takes_its_wait_once() {
    let mut notes: Vec<(u64, Waited)> = vec![(7, Waited { text: "waited 1.0 s for the bark to finish speaking".to_owned(), secs: 1.0 })];
    assert!(take_note(&mut notes, 3).is_none(), "another client's call has no wait");
    assert_eq!(take_note(&mut notes, 7).map(|w| w.text).as_deref(), Some("waited 1.0 s for the bark to finish speaking"));
    assert!(take_note(&mut notes, 7).is_none(), "a wait is used once");
}

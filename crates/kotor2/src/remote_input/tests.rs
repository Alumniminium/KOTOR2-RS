use super::*;

#[test]
fn key_names_map_to_key_codes() {
    assert_eq!(key_code("KeyW"), Some(KeyCode::KeyW));
    assert_eq!(key_code("w"), Some(KeyCode::KeyW));
    assert_eq!(key_code("Digit7"), Some(KeyCode::Digit7));
    assert_eq!(key_code("F5"), Some(KeyCode::F5));
    assert_eq!(key_code("esc"), Some(KeyCode::Escape));
    assert_eq!(key_code("ArrowUp"), Some(KeyCode::ArrowUp));
    assert_eq!(key_code("nonsense"), None);
}

#[test]
fn key_press_holds_modifiers_around_the_key() {
    // A key_press is a key_down and, once its game-time hold has run, a key_up.
    let mods = ["shift".to_owned()];
    let steps = plan_key_down("KeyA", &mods).unwrap().into_iter().chain(plan_key_up("KeyA", &mods).unwrap());
    let events: Vec<(f64, ButtonState, KeyCode)> = steps
        .map(|s| match &s.event {
            Inject::Key { code, state, .. } => (s.at_ms, *state, *code),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    assert_eq!(
        events,
        vec![
            (0.0, ButtonState::Pressed, KeyCode::ShiftLeft),
            (0.0, ButtonState::Pressed, KeyCode::KeyA),
            (0.0, ButtonState::Released, KeyCode::KeyA),
            (0.0, ButtonState::Released, KeyCode::ShiftLeft),
        ]
    );
    assert!(plan_key_down("KeyA", &["hyper".to_owned()]).is_err());
    assert!(plan_key_down("NoSuchKey", &[]).is_err());
}

#[test]
fn typed_text_carries_the_characters_and_shifts_capitals() {
    let steps = plan_type_text("Hi", Some(40)).unwrap();
    let texts: Vec<String> = steps
        .iter()
        .filter_map(|s| match &s.event {
            Inject::Key {
                text: Some(t),
                state: ButtonState::Pressed,
                ..
            } => Some(t.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(texts, vec!["H".to_owned(), "i".to_owned()]);
    assert!(steps.iter().any(|s| matches!(
        &s.event,
        Inject::Key {
            code: KeyCode::ShiftLeft,
            state: ButtonState::Pressed,
            ..
        }
    )));
    assert!(plan_type_text("\u{e9}", None).is_err());
}

#[test]
fn clicks_move_then_press_and_release() {
    let steps = plan_mouse_click(10.0, 20.0, None, Some(2), Some(30));
    assert_eq!(steps[0].event, Inject::Cursor(Vec2::new(10.0, 20.0)));
    let presses = steps
        .iter()
        .filter(|s| {
            matches!(
                s.event,
                Inject::Button {
                    state: ButtonState::Pressed,
                    ..
                }
            )
        })
        .count();
    let releases = steps
        .iter()
        .filter(|s| {
            matches!(
                s.event,
                Inject::Button {
                    state: ButtonState::Released,
                    ..
                }
            )
        })
        .count();
    assert_eq!((presses, releases), (2, 2));
    assert!(
        steps
            .windows(2)
            .all(|w| w[0].at_ms <= w[1].at_ms || w[1].at_ms == 0.0)
    );
}

#[test]
fn drags_interpolate_to_the_end() {
    let steps = plan_mouse_drag(
        [0.0, 0.0],
        [100.0, 50.0],
        Some(MouseButtonName::Right),
        Some(4),
    );
    let last_cursor = steps.iter().rev().find_map(|s| match s.event {
        Inject::Cursor(p) => Some(p),
        _ => None,
    });
    assert_eq!(last_cursor, Some(Vec2::new(100.0, 50.0)));
    assert!(steps.iter().any(|s| s.event
        == Inject::Button {
            button: MouseButton::Right,
            state: ButtonState::Released
        }));
}

#[test]
fn the_queue_releases_events_when_due() {
    let mut q = InputQueue::default();
    q.push(10.0, plan_mouse_move(1.0, 2.0));
    q.push(
        10.0,
        vec![Step {
            at_ms: 500.0,
            event: Inject::Wheel { x: 0.0, y: 1.0 },
        }],
    );
    assert!(q.due(10.0).len() == 1, "the move is due now");
    assert!(q.due(10.4).is_empty(), "the wheel is 500 ms later");
    assert_eq!(q.due(10.6).len(), 1);
    assert!(q.is_idle());
}

/// Runs the real delivery system on a headless App and checks what the
/// app's input resources report: the same path as a window's events.
#[test]
fn delivered_key_events_drive_button_input() {
    use bevy::input::InputPlugin;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .add_message::<CursorMoved>()
        .init_resource::<InputQueue>()
        .add_systems(Update, deliver_input);
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    app.update();
    let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
    app.world_mut()
        .resource_mut::<InputQueue>()
        .push(now, plan_key_down("KeyW", &[]).unwrap());
    app.update();
    app.update();
    let keys = app.world().resource::<ButtonInput<KeyCode>>();
    assert!(keys.pressed(KeyCode::KeyW), "KeyW is held after key_down");
    app.world_mut()
        .resource_mut::<InputQueue>()
        .push(now, plan_key_up("KeyW", &[]).unwrap());
    app.update();
    app.update();
    assert!(
        !app.world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyW),
        "KeyW is released after key_up"
    );
}

#[test]
fn printable_keys_type_their_character() {
    let texts = |steps: &[Step]| -> Vec<String> {
        steps
            .iter()
            .filter_map(|s| match &s.event {
                Inject::Key {
                    text: Some(t),
                    state: ButtonState::Pressed,
                    ..
                } => Some(t.clone()),
                _ => None,
            })
            .collect()
    };
    assert_eq!(texts(&plan_key_down("KeyA", &[]).unwrap()), vec!["a"]);
    assert_eq!(texts(&plan_key_down("KeyA", &["shift".to_owned()]).unwrap()), vec!["A"]);
    assert_eq!(texts(&plan_key_down("Digit7", &[]).unwrap()), vec!["7"]);
    assert_eq!(texts(&plan_key_down("Space", &[]).unwrap()), vec![" "]);
    assert!(texts(&plan_key_down("KeyA", &["ctrl".to_owned()]).unwrap()).is_empty(), "ctrl is a shortcut");
    assert!(texts(&plan_key_down("KeyA", &["super".to_owned()]).unwrap()).is_empty(), "super is a shortcut");
    assert!(texts(&plan_key_down("Enter", &[]).unwrap()).is_empty(), "Enter types nothing here");
    let logical = plan_key_down("KeyA", &["shift".to_owned()]).unwrap();
    assert!(logical.iter().any(
        |s| matches!(&s.event, Inject::Key { logical: Key::Character(c), .. } if c.as_str() == "A")
    ));
}

#[test]
fn typed_text_is_capped() {
    assert!(plan_type_text(&"a".repeat(200), None).is_ok());
    assert!(plan_type_text(&"a".repeat(201), None).is_err());
}

#[test]
fn an_abort_releases_what_the_window_holds() {
    use bevy::input::InputPlugin;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .add_message::<CursorMoved>()
        .init_resource::<InputQueue>()
        .add_systems(Update, deliver_input);
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    app.update();
    let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
    let mut queue_plan = plan_key_down("KeyW", &[]).unwrap();
    queue_plan.push(Step {
        at_ms: 0.0,
        event: Inject::Button {
            button: MouseButton::Left,
            state: ButtonState::Pressed,
        },
    });
    app.world_mut()
        .resource_mut::<InputQueue>()
        .push(now, queue_plan);
    app.update();
    app.update();
    assert!(
        app.world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyW),
        "KeyW is held"
    );
    assert!(
        app.world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left),
        "the button is held"
    );

    // A second plan is still pending when the request is aborted: it is dropped.
    app.world_mut()
        .resource_mut::<InputQueue>()
        .push(now + 60.0, plan_mouse_move(5.0, 5.0));
    let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
    app.world_mut().resource_mut::<InputQueue>().abort(now);
    app.update();
    app.update();
    assert!(
        !app.world()
            .resource::<ButtonInput<KeyCode>>()
            .pressed(KeyCode::KeyW),
        "KeyW is released by the abort"
    );
    assert!(
        !app.world()
            .resource::<ButtonInput<MouseButton>>()
            .pressed(MouseButton::Left),
        "the button is released by the abort"
    );
    assert!(
        app.world().resource::<InputQueue>().is_idle(),
        "the dropped plan does not come back"
    );
}

#[test]
fn holds_drags_and_typing_are_capped() {
    let last = |steps: &[Step]| steps.iter().map(|s| s.at_ms).fold(0.0, f64::max);
    assert!(hold_seconds(Some(3_600_000)) <= f64::from(MAX_HOLD_MS) / 1000.0, "a key is held at most {MAX_HOLD_MS} ms");
    assert_eq!(hold_seconds(None), 0.06, "a key_press holds 60 ms by default");
    let clicked = plan_mouse_click(1.0, 1.0, None, Some(1), Some(u32::MAX));
    assert!(
        last(&clicked) <= f64::from(MAX_HOLD_MS) + SETTLE_MS,
        "a button is held at most {MAX_HOLD_MS} ms"
    );
    let typed = plan_type_text("ab", Some(600_000)).unwrap();
    let second = typed
        .iter()
        .filter_map(|s| match &s.event {
            Inject::Key {
                text: Some(_),
                state: ButtonState::Pressed,
                ..
            } => Some(s.at_ms),
            _ => None,
        })
        .nth(1)
        .expect("two characters");
    assert!(
        second <= f64::from(MAX_TYPE_DELAY_MS),
        "characters are at most {MAX_TYPE_DELAY_MS} ms apart"
    );
    let dragged = plan_mouse_drag([0.0, 0.0], [10.0, 10.0], None, Some(100_000));
    let moves = dragged
        .iter()
        .filter(|s| matches!(s.event, Inject::Cursor(_)))
        .count();
    assert!(
        moves <= MAX_DRAG_STEPS as usize + 1,
        "a drag passes through at most {MAX_DRAG_STEPS} positions"
    );
}

#[test]
fn pointer_events_move_the_window_cursor() {
    use bevy::input::InputPlugin;
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, InputPlugin))
        .add_message::<CursorMoved>()
        .init_resource::<InputQueue>()
        .add_systems(Update, deliver_input);
    app.world_mut()
        .spawn((Window::default(), bevy::window::PrimaryWindow));
    app.update();
    let now = app.world().resource::<Time<Real>>().elapsed_secs_f64();
    app.world_mut()
        .resource_mut::<InputQueue>()
        .push(now, plan_mouse_move(640.0, 360.0));
    app.update();
    let mut windows = app.world_mut().query::<&Window>();
    let window = windows.single(app.world()).unwrap();
    assert_eq!(window.cursor_position(), Some(Vec2::new(640.0, 360.0)));
}

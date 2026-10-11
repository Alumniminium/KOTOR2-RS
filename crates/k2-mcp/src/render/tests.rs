use super::*;
use k2_world::control::{Member, Nearby, Observation};
use serde_json::json;

#[test]
fn floats_are_rounded_to_two_decimals_and_whole_numbers_print_as_integers() {
    assert_eq!(round_floats(json!({ "a": 1.2345, "b": 2.0, "c": [0.126, 3] })), json!({ "a": 1.23, "b": 2, "c": [0.13, 3] }));
}

#[test]
fn a_message_that_repeats_in_the_feedback_is_printed_once() {
    let reply = Reply {
        ok: true,
        message: "Unable to reach target!".to_owned(),
        feedback: vec!["Unable to reach target!".to_owned(), "You walk.".to_owned(), "You walk.".to_owned()],
        data: None,
    };
    assert_eq!(plain(&reply), "Unable to reach target!\nYou walk.");
}

#[test]
fn data_is_one_line_of_json_without_the_envelope() {
    let reply = Reply::ok_with("moved", json!({ "a": 1.5, "b": null }));
    assert_eq!(plain(&reply), r#"{"a":1.5,"b":null}"#);
}

#[test]
fn a_verb_without_data_answers_with_its_message() {
    assert_eq!(plain(&Reply::ok("typed 5 characters")), "typed 5 characters");
}

#[test]
fn observe_lists_one_line_per_object_and_leaves_out_empty_and_default_fields() {
    let obs = Observation {
        module: "001ebo".to_owned(),
        time: 1.5,
        leader: Some(3),
        party: vec![Member {
            id: 3,
            tag: String::new(),
            name: "Atton".to_owned(),
            leader: true,
            level: 5,
            xp: 100,
            next_level_xp: 0,
            hp: 40,
            max_hp: 50,
            fp: 0,
            max_fp: 0,
            dead: false,
            in_combat: false,
            position: [1.0, 2.0, 0.0],
            facing: 0.0,
            conditions: Vec::new(),
            equipment: Vec::new(),
            powers: Vec::new(),
        }],
        nearby: vec![Nearby {
            id: 9,
            tag: "door01".to_owned(),
            name: String::new(),
            kind: "door".to_owned(),
            position: [3.0, 4.0, 0.0],
            distance: 3.5,
            hostile: None,
            dead: None,
            locked: Some(true),
            open: Some(false),
            useable: true,
            scripted_use: false,
            conversation: false,
            trapped: false,
            exit_to: None,
        }],
        ..Default::default()
    };
    let value = serde_json::to_value(&obs).unwrap();
    assert_eq!(
        observe(&value, false).unwrap(),
        "001ebo t=1.50 leader=3\n3 \"Atton\" lvl=5 hp=40/50 leader\n9 door01 (door, closed, locked) - 3.5 m ahead and to your left"
    );
    assert_eq!(
        observe(&value, true).unwrap(),
        "001ebo t=1.50 leader=3\n3 \"Atton\" lvl=5 hp=40/50 pos=(1.00,2.00,0.00) leader\n9 door01 (door, closed, locked) - 3.5 m ahead and to your left at=(3.00,4.00,0.00)"
    );
}

/// A party member at `position`, facing `facing` radians.
fn leader_at(position: [f32; 3], facing: f32) -> Member {
    Member {
        id: 3,
        tag: String::new(),
        name: "Atton".to_owned(),
        leader: true,
        level: 1,
        xp: 0,
        next_level_xp: 0,
        hp: 10,
        max_hp: 10,
        fp: 0,
        max_fp: 0,
        dead: false,
        in_combat: false,
        position,
        facing,
        conditions: Vec::new(),
        equipment: Vec::new(),
        powers: Vec::new(),
    }
}

/// A nearby object of `kind` at `position`, `distance` metres from the leader.
fn near(id: u32, kind: &str, position: [f32; 3], distance: f32) -> Nearby {
    Nearby {
        id,
        tag: format!("tag{id}"),
        name: String::new(),
        kind: kind.to_owned(),
        position,
        distance,
        hostile: None,
        dead: None,
        locked: None,
        open: None,
        useable: false,
        scripted_use: false,
        conversation: false,
        trapped: false,
        exit_to: None,
    }
}

/// observe's text for one leader and the nearby objects given.
fn observe_with(leader: Member, nearby: Vec<Nearby>, verbose: bool) -> String {
    let obs = Observation { module: "001ebo".to_owned(), leader: Some(3), party: vec![leader], nearby, ..Default::default() };
    observe(&serde_json::to_value(&obs).unwrap(), verbose).unwrap()
}

#[test]
fn scenery_and_helpers_are_left_out_and_interactables_stay() {
    let mut chair = near(1, "placeable", [1.0, 0.0, 0.0], 1.0);
    chair.tag = "plc_chair".to_owned();
    let mut desk_door = near(2, "door", [2.0, 0.0, 0.0], 2.0);
    desk_door.useable = false;
    let mut computer = near(3, "placeable", [3.0, 0.0, 0.0], 3.0);
    computer.useable = true;
    computer.conversation = true;
    computer.name = "Medical Computer".to_owned();
    let mut helper = near(4, "trigger", [4.0, 0.0, 0.0], 4.0);
    helper.tag = "k_trans_abort".to_owned();
    let mut door = near(5, "door", [5.0, 0.0, 0.0], 5.0);
    door.useable = true;
    door.open = Some(true);
    let text = observe_with(leader_at([0.0, 0.0, 0.0], 0.0), vec![chair, desk_door, computer, helper, door], false);
    let lines: Vec<&str> = text.lines().collect();
    assert_eq!(lines.len(), 4, "the header, the leader and two interactables: {text}");
    assert_eq!(lines[2], "3 Medical Computer (placeable, talks) - 3.0 m in front of you");
    assert_eq!(lines[3], "5 tag5 (door, open) - 5.0 m in front of you");
}

#[test]
fn directions_follow_the_leader_facing() {
    let left = near(10, "item", [0.0, 2.0, 0.0], 2.0);
    let behind = near(11, "item", [-2.0, 0.0, 0.0], 2.0);
    let right = near(12, "item", [0.0, -2.0, 0.0], 2.0);
    let text = observe_with(leader_at([0.0, 0.0, 0.0], 0.0), vec![left, behind, right], false);
    assert!(text.contains("10 tag10 (item) - 2.0 m to your left"), "{text}");
    assert!(text.contains("11 tag11 (item) - 2.0 m behind you"), "{text}");
    assert!(text.contains("12 tag12 (item) - 2.0 m to your right"), "{text}");

    // Facing north (+Y): straight ahead is +Y, and +X is to the right.
    let ahead = near(13, "item", [0.0, 2.0, 0.0], 2.0);
    let east = near(14, "item", [2.0, 0.0, 0.0], 2.0);
    let text = observe_with(leader_at([0.0, 0.0, 0.0], std::f32::consts::FRAC_PI_2), vec![ahead, east], false);
    assert!(text.contains("13 tag13 (item) - 2.0 m in front of you"), "{text}");
    assert!(text.contains("14 tag14 (item) - 2.0 m to your right"), "{text}");
}

#[test]
fn a_height_difference_of_more_than_one_and_a_half_metres_is_named() {
    let above = near(20, "item", [1.0, 0.0, 2.0], 1.0);
    let below = near(21, "item", [1.0, 0.0, -2.0], 1.0);
    let level = near(22, "item", [1.0, 0.0, 1.0], 1.0);
    let text = observe_with(leader_at([0.0, 0.0, 0.0], 0.0), vec![above, below, level], false);
    assert!(text.contains("20 tag20 (item) - 1.0 m in front of you, above"), "{text}");
    assert!(text.contains("21 tag21 (item) - 1.0 m in front of you, below"), "{text}");
    assert!(text.lines().any(|line| line == "22 tag22 (item) - 1.0 m in front of you"), "a height within 1.5 m is not named: {text}");
}

#[test]
fn observe_refuses_data_that_is_not_an_observation() {
    assert!(observe(&json!("text"), false).is_none());
}

#[test]
fn an_open_window_is_the_first_line_of_observe() {
    let obs = Observation { module: "001ebo".to_owned(), leader: Some(3), party: vec![leader_at([0.0, 0.0, 0.0], 0.0)], ..Default::default() };
    let mut data = serde_json::to_value(&obs).unwrap();
    data["open"] = json!("open: container window 'Corpse' (2 items)");
    let text = observe(&data, false).unwrap();
    assert_eq!(text.lines().next(), Some("open: container window 'Corpse' (2 items)"), "{text}");
    assert!(text.lines().nth(1).is_some_and(|line| line.starts_with("001ebo")), "the header follows: {text}");
}

#[test]
fn a_call_that_waited_for_speech_keeps_its_wait_message_ahead_of_its_data() {
    let reply = Reply::ok_with("waited 3.2 s for Kreia to finish speaking; the call ran", json!({ "object": 9, "waited_s": 3.2 }));
    let text = plain(&reply);
    assert!(text.starts_with("waited 3.2 s for Kreia to finish speaking; the call ran\n"), "{text}");
    assert!(text.contains(r#""waited_s":3.2"#), "the data follows: {text}");
}

#[test]
fn a_reply_without_a_wait_keeps_its_data_alone() {
    let reply = Reply::ok_with("moved", json!({ "object": 9 }));
    assert_eq!(plain(&reply), r#"{"object":9}"#);
}

#[test]
fn game_status_is_trimmed_unless_verbose() {
    let data = json!({
        "running": true,
        "mode": "windowed",
        "run": {
            "pid": 1,
            "run_id": "r",
            "uptime_s": 1.23456,
            "start": "new game",
            "build": { "stamp": "S1", "describe": "d", "binary": "/b", "warnings": 0, "bytes": 1 },
            "log_file": "/l",
            "socket": "/s"
        },
        "game": {
            "phase": "game",
            "log_filter": "info",
            "window": { "width": 1280.0, "height": 720.0, "physical_width": 1280, "settled": true },
            "trace_file": null
        },
        "last_build": { "stamp": "S1" },
        "last_exit": null
    });
    let short = status(&data, false);
    assert_eq!(short["run"]["build"], json!("S1"));
    assert_eq!(short["run"]["uptime_s"], json!(1.23));
    assert!(short["run"].get("socket").is_none());
    assert!(short.get("last_build").is_none(), "the running build is not repeated");
    assert!(short["game"].get("log_filter").is_none());
    assert!(short["game"].get("trace_file").is_none());
    assert_eq!(short["game"]["window"]["height"], json!(720));
    assert!(short["game"]["window"].get("physical_width").is_none());

    let full = status(&data, true);
    assert_eq!(full["run"]["socket"], json!("/s"));
    assert_eq!(full["game"]["log_filter"], json!("info"));
    assert_eq!(full["last_build"]["stamp"], json!("S1"));
}

#[test]
fn a_stopped_game_shows_its_last_build_by_stamp() {
    let data = json!({ "running": false, "run": null, "last_build": { "stamp": "S0", "bytes": 9 } });
    assert_eq!(status(&data, false)["last_build"], json!("S0"));
}

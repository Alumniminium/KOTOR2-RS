use super::*;

#[test]
fn commands_round_trip_on_the_wire() {
    let cases = [
        r#"{"cmd":"observe"}"#,
        r#"{"cmd":"observe","radius":5.5}"#,
        r#"{"cmd":"move_to","point":[1.0,2.0,0.5]}"#,
        r#"{"cmd":"interact","object":4097}"#,
        r#"{"cmd":"choose_reply","index":2}"#,
        r#"{"cmd":"continue_dialog"}"#,
        r#"{"cmd":"use_skill","skill":"unlock","target":12}"#,
        r#"{"cmd":"level_up","creature":1,"skills":[{"skill":0,"ranks":1}]}"#,
        r#"{"cmd":"wait_until","condition":"combat_end","timeout":30}"#,
        r#"{"cmd":"save","slot":"mcp-a"}"#,
        r#"{"cmd":"debug_set_global","name":"K","kind":"number","value":"3"}"#,
        r#"{"cmd":"key_press","key":"KeyW","modifiers":["shift"],"hold_ms":80}"#,
        r#"{"cmd":"type_text","text":"Hi there"}"#,
        r#"{"cmd":"mouse_click","x":640.5,"y":360.0,"button":"right","clicks":2}"#,
        r#"{"cmd":"mouse_drag","from":[10.0,20.0],"to":[30.0,40.0],"steps":8}"#,
        r#"{"cmd":"scroll","lines_y":-3.0}"#,
        r#"{"cmd":"screenshot","path":"/tmp/x.png"}"#,
        r#"{"cmd":"set_log_filter","filter":"info,kotor2=debug"}"#,
        r#"{"cmd":"trace_start","path":"t.json","filter":"info,kotor2=trace"}"#,
        r#"{"cmd":"trace_stop"}"#,
        r#"{"cmd":"info"}"#,
        r#"{"cmd":"pick","x":640.0,"y":360.0}"#,
        r#"{"cmd":"project","object":4097}"#,
    ];
    for text in cases {
        let cmd: Command = serde_json::from_str(text).unwrap_or_else(|e| panic!("{text}: {e}"));
        let again = serde_json::to_string(&cmd).unwrap();
        let back: Command = serde_json::from_str(&again).unwrap();
        assert_eq!(serde_json::to_value(&cmd).unwrap(), serde_json::to_value(&back).unwrap(), "{text}");
    }
    assert!(serde_json::from_str::<Command>(r#"{"cmd":"fly"}"#).is_err());
    assert!(serde_json::from_str::<Command>(r#"{"cmd":"advance_time","seconds":5}"#).is_err(), "test-only verbs are gone");
    assert!(serde_json::from_str::<Command>(r#"{"cmd":"new_game"}"#).is_err(), "test-only verbs are gone");
}

#[test]
fn host_verbs_are_not_world_verbs() {
    let host: Command = serde_json::from_str(r#"{"cmd":"mouse_move","x":1.0,"y":2.0}"#).unwrap();
    assert!(host.is_host() && !host.is_read_only());
    let info: Command = serde_json::from_str(r#"{"cmd":"info"}"#).unwrap();
    assert!(info.is_host() && info.is_read_only(), "info answers during a goal");
    let observe: Command = serde_json::from_str(r#"{"cmd":"observe"}"#).unwrap();
    assert!(!observe.is_host() && observe.is_read_only());
    let pick: Command = serde_json::from_str(r#"{"cmd":"pick","x":1.0,"y":2.0}"#).unwrap();
    assert!(pick.is_host() && !pick.allowed_during_goal(), "pick moves the pointer, so it waits for a goal");
    assert_eq!(pick.verb(), "pick");
}

#[test]
fn a_goal_lets_the_read_only_and_the_quiet_host_verbs_through() {
    let parse = |text: &str| serde_json::from_str::<Command>(text).unwrap();
    for text in [
        r#"{"cmd":"observe"}"#,
        r#"{"cmd":"info"}"#,
        r#"{"cmd":"project","object":1}"#,
        r#"{"cmd":"screenshot"}"#,
        r#"{"cmd":"set_log_filter","filter":"info"}"#,
        r#"{"cmd":"trace_start"}"#,
        r#"{"cmd":"trace_stop"}"#,
        r#"{"cmd":"stop"}"#,
        r#"{"cmd":"quit"}"#,
    ] {
        assert!(parse(text).allowed_during_goal(), "{text}");
    }
    for text in [r#"{"cmd":"key_press","key":"KeyW"}"#, r#"{"cmd":"mouse_click","x":1.0,"y":2.0}"#, r#"{"cmd":"save","slot":"mcp-a"}"#] {
        assert!(!parse(text).allowed_during_goal(), "{text}");
    }
}

#[test]
fn save_slots_need_the_mcp_prefix() {
    assert!(check_save_slot("mcp-test_1").is_ok());
    assert!(check_save_slot("save-0001").is_err());
    assert!(check_save_slot("mcp-").is_err());
    assert!(check_save_slot("mcp-a/b").is_err());
    assert!(check_save_slot("mcp-../x").is_err());
}

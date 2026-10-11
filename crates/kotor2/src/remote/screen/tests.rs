use super::*;

fn cmd(text: &str) -> Command {
    serde_json::from_str(text).expect("a command")
}

fn container(items: usize) -> Screen {
    Screen::Container { name: "Corpse".to_owned(), items }
}

fn window(menu: &str) -> Screen {
    Screen::Window(menu.to_owned())
}

#[test]
fn a_refused_verb_names_the_open_container_and_how_to_close_it() {
    let move_to = cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#);
    assert_eq!(
        refusal(&container(2), &move_to).as_deref(),
        Some("refused: the container window 'Corpse' is open; close it with take_all, or key_press Escape")
    );
}

#[test]
fn the_container_takes_take_all_and_take_item_and_nothing_that_moves_the_party() {
    assert!(allows(&container(1), &cmd(r#"{"cmd":"take_all","object":5}"#)));
    assert!(allows(&container(1), &cmd(r#"{"cmd":"take_item","container":5,"item":6}"#)));
    assert!(!allows(&container(1), &cmd(r#"{"cmd":"interact","object":5}"#)), "the party does not walk off from an open container");
}

#[test]
fn a_conversation_takes_its_dialog_verbs_only() {
    let talk = Screen::Conversation;
    assert!(allows(&talk, &cmd(r#"{"cmd":"choose_reply","index":0}"#)));
    assert!(allows(&talk, &cmd(r#"{"cmd":"continue_dialog"}"#)));
    assert!(allows(&talk, &cmd(r#"{"cmd":"end_dialog"}"#)));
    assert_eq!(
        refusal(&talk, &cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#)).as_deref(),
        Some("refused: a conversation is running; answer it with choose_reply or continue_dialog")
    );
}

#[test]
fn a_paused_game_and_the_other_blockers_name_their_way_out() {
    let walk = cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#);
    assert_eq!(refusal(&Screen::Paused, &walk).as_deref(), Some("refused: the game is paused; resume with key_press Space"));
    assert_eq!(refusal(&Screen::Movie, &walk).as_deref(), Some("refused: a movie is playing; skip it with key_press Escape"));
    assert_eq!(
        refusal(&Screen::Loading, &walk).as_deref(),
        Some("refused: the game is loading a module; wait for it to finish (observe shows the module)")
    );
    assert_eq!(
        refusal(&Screen::NotInGame, &walk).as_deref(),
        Some("refused: no game is running (the main menu is up); start a game first (game_start)")
    );
}

#[test]
fn the_pause_menu_takes_nothing_but_the_escape() {
    let equip = cmd(r#"{"cmd":"equip","item":5}"#);
    assert_eq!(refusal(&window("optionsingame"), &equip).as_deref(), Some("refused: the pause menu is open; close it with key_press Escape"));
    assert!(allows(&window("inventory"), &equip), "the inventory window equips");
    assert!(!allows(&window("inventory"), &cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#)));
}

#[test]
fn reads_host_verbs_and_stop_run_whatever_is_open() {
    assert!(allows(&Screen::Movie, &cmd(r#"{"cmd":"observe"}"#)));
    assert!(allows(&container(3), &cmd(r#"{"cmd":"key_press","key":"KeyW"}"#)), "raw input is delivered, and the reply says where it went");
    assert!(allows(&Screen::Movie, &cmd(r#"{"cmd":"stop"}"#)));
    assert!(allows(&Screen::Loading, &cmd(r#"{"cmd":"save","slot":"mcp-a"}"#)), "save has its own checks");
    assert!(allows(&Screen::Play, &cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#)));
    assert!(refusal(&Screen::Play, &cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#)).is_none());
}

/// A running game with nothing open.
fn running() -> Look<'static> {
    Look { in_game: true, loading: false, movie: false, game_over: false, conversation: false, box_up: false, level_up: false, menu: None, container: None, paused: false }
}

#[test]
fn a_message_box_takes_continue_and_nothing_that_moves_the_party() {
    let walk = cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#);
    assert!(allows(&Screen::MessageBox, &cmd(r#"{"cmd":"continue_dialog"}"#)), "OK advances the box");
    assert_eq!(
        refusal(&Screen::MessageBox, &walk).as_deref(),
        Some("refused: a message box is open; close it with continue_dialog, or key_press Escape")
    );
    assert_eq!(open_line(&Screen::MessageBox).as_deref(), Some("open: message box"));
    assert_eq!(screen_now(&Look { conversation: true, box_up: true, ..running() }), Screen::MessageBox, "a box is checked before the conversation");
    assert_eq!(screen_now(&Look { conversation: true, ..running() }), Screen::Conversation);
}

#[test]
fn the_game_over_and_level_up_screens_take_what_they_are_for() {
    let walk = cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#);
    assert_eq!(refusal(&Screen::GameOver, &walk).as_deref(), Some("refused: the game is over (the party is down); load a save with load, or quit"));
    assert!(allows(&Screen::GameOver, &cmd(r#"{"cmd":"load","slot":"restart"}"#)), "load is how a dead party goes on");
    assert!(allows(&Screen::LevelUp, &cmd(r#"{"cmd":"level_up","creature":5}"#)));
    assert_eq!(
        refusal(&Screen::LevelUp, &walk).as_deref(),
        Some("refused: the level-up screen is open; finish it with level_up, or key_press Escape")
    );
    assert_eq!(screen_now(&Look { game_over: true, paused: true, ..running() }), Screen::GameOver);
    assert_eq!(screen_now(&Look { level_up: true, menu: Some("levelup"), ..running() }), Screen::LevelUp);
    assert_eq!(open_line(&Screen::LevelUp).as_deref(), Some("open: level-up screen"));
}

#[test]
fn screen_now_checks_the_most_specific_state_first() {
    assert_eq!(screen_now(&Look { in_game: false, ..running() }), Screen::NotInGame);
    assert_eq!(screen_now(&Look { movie: true, paused: true, ..running() }), Screen::Movie);
    assert_eq!(screen_now(&Look { loading: true, paused: true, ..running() }), Screen::Loading);
    assert_eq!(screen_now(&Look { conversation: true, menu: Some("container"), ..running() }), Screen::Conversation, "a conversation comes before a window");
    assert_eq!(screen_now(&Look { menu: Some("container"), container: Some(("Corpse".to_owned(), 2)), ..running() }), container(2));
    assert_eq!(screen_now(&Look { menu: Some("optionsingame"), paused: true, ..running() }), window("optionsingame"));
    assert_eq!(screen_now(&Look { paused: true, ..running() }), Screen::Paused);
    assert_eq!(screen_now(&running()), Screen::Play);
}

#[test]
fn observe_puts_the_open_window_first() {
    assert_eq!(open_line(&container(2)).as_deref(), Some("open: container window 'Corpse' (2 items)"));
    assert_eq!(open_line(&window("optionsingame")).as_deref(), Some("open: pause menu"));
    assert_eq!(open_line(&window("inventory")).as_deref(), Some("open: inventory window"));
    assert_eq!(open_line(&Screen::Play), None, "nothing is open, nothing is reported");
}

#[test]
fn raw_input_says_which_window_has_focus() {
    assert_eq!(focus(&container(2)).as_deref(), Some("container"));
    assert_eq!(focus(&window("optionsingame")).as_deref(), Some("pause menu"));
    assert_eq!(focus(&window("inventory")).as_deref(), Some("inventory"));
    assert_eq!(focus(&Screen::Play), None);
    assert!(is_raw_input(&cmd(r#"{"cmd":"key_press","key":"KeyW"}"#)));
    assert!(!is_raw_input(&cmd(r#"{"cmd":"move_to","point":[1.0,2.0,0.0]}"#)));
}

use std::io::Write;
use super::*;

#[test]
fn editor_button_keeps_the_keyboard_order() {
    let dir = std::env::var("K2_INSTALL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(std::io::stderr(), "SKIP editor_button_keeps_the_keyboard_order: no install (set K2_INSTALL)");
        return;
    };
    let res = k2_res::Resources::open(install).unwrap();
    let mut d = super::super::layout::load(&res, "mainmenu8x6_p").expect("main menu layout");
    assert!(editor_slot(&mut d));
    let at = |tag: &str| {
        d.index(tag)
            .unwrap_or_else(|| panic!("{tag} in the layout")) as i32
    };
    let moves = |tag: &str| d.controls[at(tag) as usize].move_to;
    // Top to bottom: Options, Editor, Exit.
    assert_eq!(moves("BTN_OPTIONS").down, at("BTN_EDITOR"));
    assert_eq!(moves("BTN_EDITOR").up, at("BTN_OPTIONS"));
    assert_eq!(moves("BTN_EDITOR").down, at("BTN_EXIT"));
    assert_eq!(moves("BTN_EXIT").up, at("BTN_EDITOR"));
    assert_eq!(moves("BTN_EXIT").down, -1);
    let top = |tag: &str| d.controls[at(tag) as usize].rect[1];
    assert!(top("BTN_OPTIONS") < top("BTN_EDITOR") && top("BTN_EDITOR") < top("BTN_EXIT"));
}

#[test]
fn more_games_is_hidden_when_there_is_no_editor_slot() {
    let dir = std::env::var("K2_INSTALL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(
            std::io::stderr(),
            "SKIP more_games_is_hidden_when_there_is_no_editor_slot: no install (set K2_INSTALL)"
        );
        return;
    };
    let res = k2_res::Resources::open(install).unwrap();
    let mut d = super::super::layout::load(&res, "mainmenu8x6_p").expect("main menu layout");
    // Without BTN_EXIT there is nothing to swap with.
    let exit = d.index("BTN_EXIT").expect("BTN_EXIT in the layout");
    d.controls[exit].tag = "BTN_QUIT".into();
    let mut scr = GuiScreen::new(d, Placement::Center);
    scr.set_visible("BTN_MOREGAMES", true);
    assert!(scr.is_visible("BTN_MOREGAMES"));
    add_editor_button(&mut scr, true);
    assert!(
        !scr.is_visible("BTN_MOREGAMES"),
        "the unused More Games button stays visible"
    );
    assert!(
        scr.def.index("BTN_EDITOR").is_none(),
        "the layout was swapped without both buttons"
    );
}

#[test]
fn no_editor_button_when_the_editor_is_not_allowed() {
    let dir = std::env::var("K2_INSTALL")
        .unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../..").into());
    let Ok(install) = k2_res::Install::discover(dir) else {
        let _ = writeln!(
            std::io::stderr(),
            "SKIP no_editor_button_when_the_editor_is_not_allowed: no install (set K2_INSTALL)"
        );
        return;
    };
    let res = k2_res::Resources::open(install).unwrap();
    let d = super::super::layout::load(&res, "mainmenu8x6_p").expect("main menu layout");
    let mut scr = GuiScreen::new(d, Placement::Center);
    scr.set_visible("BTN_MOREGAMES", true);
    add_editor_button(&mut scr, false);
    assert!(
        scr.def.index("BTN_EDITOR").is_none(),
        "a control run has no Editor button"
    );
    assert!(scr.def.index("BTN_EXIT").is_some(), "Exit keeps its place");
    assert!(
        !scr.is_visible("BTN_MOREGAMES"),
        "the unused slot is hidden, as without an editor slot"
    );
}

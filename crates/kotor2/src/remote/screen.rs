//! What is on screen that takes the player's input, and which semantic verbs
//! may run while it is up. remote.rs reads the game state and passes it in as a
//! [`Look`]; everything here is plain data, so it is tested without a window.
//!
//! A verb that the screen does not take is refused at once, naming what is open
//! and how to close it. Nothing is queued, so the call never waits on a window.
//! Reads and host verbs always run: they report the screen, or deliver input
//! to it (the reply then says which window got the input).

use k2_world::control::Command;

#[cfg(test)]
mod tests;

/// What the player's input is going to. [`screen_now`] checks in this order.
#[derive(Debug, Clone, PartialEq)]
pub enum Screen {
    /// No game is running: the main menu.
    NotInGame,
    /// A module or a save is being read in.
    Loading,
    /// A movie owns the screen.
    Movie,
    /// The party is down: the game over screen is due or up.
    GameOver,
    /// A conversation is running.
    Conversation,
    /// A message box is up: a conversation line with no real reply (a door's box).
    MessageBox,
    /// The level-up screen is up.
    LevelUp,
    /// The container window, showing `items` items of the container `name`.
    Container { name: String, items: usize },
    /// Another window, by its menu name (`optionsingame` is the pause menu).
    Window(String),
    /// The game is paused, with no window open.
    Paused,
    /// Nothing blocks play.
    Play,
}

/// The game state a [`Screen`] is worked out from (read in remote.rs).
pub struct Look<'a> {
    pub in_game: bool,
    pub loading: bool,
    pub movie: bool,
    pub game_over: bool,
    pub conversation: bool,
    /// The conversation on screen is a message box (see `ConversationView::boxed`).
    pub box_up: bool,
    pub level_up: bool,
    /// The open menu, as OpenMenu names it.
    pub menu: Option<&'a str>,
    /// The container window's object name and item count, when it is the open window.
    pub container: Option<(String, usize)>,
    pub paused: bool,
}

/// What is up, from the state of the game.
pub fn screen_now(look: &Look) -> Screen {
    if !look.in_game {
        Screen::NotInGame
    } else if look.movie {
        Screen::Movie
    } else if look.game_over {
        Screen::GameOver
    } else if look.loading {
        Screen::Loading
    } else if look.box_up {
        Screen::MessageBox
    } else if look.conversation {
        Screen::Conversation
    } else if look.level_up {
        Screen::LevelUp
    } else if look.menu == Some("container") {
        let (name, items) = look.container.clone().unwrap_or_else(|| ("container".to_owned(), 0));
        Screen::Container { name, items }
    } else if let Some(menu) = look.menu {
        Screen::Window(menu.to_owned())
    } else if look.paused {
        Screen::Paused
    } else {
        Screen::Play
    }
}

/// What is up, for a refusal. None when nothing is.
pub fn describe(screen: &Screen) -> Option<String> {
    Some(match screen {
        Screen::Play => return None,
        Screen::NotInGame => "no game is running (the main menu is up)".to_owned(),
        Screen::Loading => "the game is loading a module".to_owned(),
        Screen::Movie => "a movie is playing".to_owned(),
        Screen::GameOver => "the game is over (the party is down)".to_owned(),
        Screen::Conversation => "a conversation is running".to_owned(),
        Screen::MessageBox => "a message box is open".to_owned(),
        Screen::LevelUp => "the level-up screen is open".to_owned(),
        Screen::Container { name, .. } => format!("the container window '{name}' is open"),
        Screen::Window(menu) if menu == "optionsingame" => "the pause menu is open".to_owned(),
        Screen::Window(menu) => format!("the {menu} window is open"),
        Screen::Paused => "the game is paused".to_owned(),
    })
}

/// How to get past what is up, after "close it with" or the like.
fn how_to_clear(screen: &Screen) -> &'static str {
    match screen {
        Screen::Container { .. } => "close it with take_all, or key_press Escape",
        Screen::Conversation => "answer it with choose_reply or continue_dialog",
        Screen::MessageBox => "close it with continue_dialog, or key_press Escape",
        Screen::Paused => "resume with key_press Space",
        Screen::Movie => "skip it with key_press Escape",
        Screen::GameOver => "load a save with load, or quit",
        Screen::LevelUp => "finish it with level_up, or key_press Escape",
        Screen::Loading => "wait for it to finish (observe shows the module)",
        Screen::NotInGame => "start a game first (game_start)",
        Screen::Window(_) => "close it with key_press Escape",
        Screen::Play => "",
    }
}

/// Whether `cmd` may run while `screen` is up.
pub fn allows(screen: &Screen, cmd: &Command) -> bool {
    if matches!(screen, Screen::Play) || cmd.is_read_only() || cmd.is_host() {
        return true;
    }
    if matches!(cmd, Command::Stop | Command::Quit | Command::Save(_) | Command::Load(_) | Command::DebugTeleport(_) | Command::DebugSetGlobal(_) | Command::DebugGiveItem(_)) {
        return true;
    }
    match (screen, cmd) {
        (Screen::Conversation, Command::ChooseReply(_) | Command::ContinueDialog | Command::EndDialog(_)) => true,
        (Screen::Container { .. }, Command::TakeItem(_) | Command::TakeAll(_)) => true,
        (Screen::LevelUp, Command::LevelUp(_)) => true,
        (Screen::MessageBox, Command::ContinueDialog) => true,
        (Screen::Window(menu), Command::Equip(_) | Command::Unequip(_) | Command::UseItem(_)) => menu != "optionsingame",
        _ => false,
    }
}

/// The refusal for a verb the screen does not take, or None when it may run.
/// The text names what is open and how to close it, for example
/// "refused: the container window 'Corpse' is open; close it with take_all, or key_press Escape".
pub fn refusal(screen: &Screen, cmd: &Command) -> Option<String> {
    if allows(screen, cmd) {
        return None;
    }
    let what = describe(screen)?;
    Some(format!("refused: {what}; {}", how_to_clear(screen)))
}

/// The line observe puts first while something is up, for example
/// "open: container window 'Corpse' (2 items)". None when nothing is.
pub fn open_line(screen: &Screen) -> Option<String> {
    Some(match screen {
        Screen::Play => return None,
        Screen::NotInGame => "open: no game (main menu)".to_owned(),
        Screen::Loading => "open: loading".to_owned(),
        Screen::Movie => "open: movie".to_owned(),
        Screen::GameOver => "open: game over".to_owned(),
        Screen::Conversation => "open: conversation".to_owned(),
        Screen::MessageBox => "open: message box".to_owned(),
        Screen::LevelUp => "open: level-up screen".to_owned(),
        Screen::Container { name, items } => format!("open: container window '{name}' ({items} items)"),
        Screen::Window(menu) if menu == "optionsingame" => "open: pause menu".to_owned(),
        Screen::Window(menu) => format!("open: {menu} window"),
        Screen::Paused => "open: paused".to_owned(),
    })
}

/// The window that raw input goes to while it has focus, named for the reply
/// ("input went to the open container window"). None when no window has focus.
pub fn focus(screen: &Screen) -> Option<String> {
    match screen {
        Screen::Container { .. } => Some("container".to_owned()),
        Screen::LevelUp => Some("level-up".to_owned()),
        Screen::GameOver => Some("game over".to_owned()),
        Screen::MessageBox => Some("message box".to_owned()),
        Screen::Window(menu) if menu == "optionsingame" => Some("pause menu".to_owned()),
        Screen::Window(menu) => Some(menu.clone()),
        _ => None,
    }
}

/// Whether `cmd` is raw input (keys, pointer, typing, wheel).
pub fn is_raw_input(cmd: &Command) -> bool {
    matches!(
        cmd,
        Command::KeyPress(_) | Command::KeyDown(_) | Command::KeyUp(_) | Command::TypeText(_) | Command::MouseMove(_) | Command::MouseClick(_) | Command::MouseDrag(_) | Command::Scroll(_)
    )
}

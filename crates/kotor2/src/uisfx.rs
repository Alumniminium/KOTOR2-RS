//! Interface sounds from guisounds.2da (label -> soundresref): button clicks,
//! hover, panel open/close, errors, inventory and notification cues.
//!
//! Anything in the UI writes `UiSfx("Clicked_Default")` and this plays it.
//! GUI button presses (`GuiClick`) click automatically.

use std::collections::HashMap;

use bevy::prelude::*;

use crate::area::GameData;
use crate::audio::Voice;
use crate::gui::GuiClick;
use crate::model::GameRes;

pub struct UiSfxPlugin;

impl Plugin for UiSfxPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<UiSfx>().init_resource::<UiSounds>().add_systems(Update, (load_table, play_ui_sfx).chain());
    }
}

/// A guisounds.2da label, e.g. "Clicked_Default", "Entered_Default".
#[derive(Message, Clone, Copy, Debug)]
pub struct UiSfx(pub &'static str);

#[derive(Resource, Default)]
struct UiSounds {
    loaded: bool,
    by_label: HashMap<String, String>,
}

fn load_table(mut sounds: ResMut<UiSounds>, game: Option<Res<GameRes>>, gd: Option<Res<GameData>>) {
    if sounds.loaded {
        return;
    }
    let (Some(game), Some(gd)) = (game, gd) else { return };
    sounds.loaded = true;
    let Ok(t) = gd.tables.get(&game.0, "guisounds") else {
        warn!("uisfx: guisounds.2da missing");
        return;
    };
    for row in 0..t.rows() {
        if let (Some(label), Some(snd)) = (t.str(row, "label"), t.str(row, "soundresref")) {
            sounds.by_label.insert(label.to_ascii_lowercase(), snd.to_ascii_lowercase());
        }
    }
    info!("uisfx: {} gui sounds", sounds.by_label.len());
}

fn play_ui_sfx(sounds: Res<UiSounds>, mut cues: MessageReader<UiSfx>, mut clicks: MessageReader<GuiClick>, mut voice: Voice) {
    let mut labels: Vec<&str> = cues.read().map(|c| c.0).collect();
    if clicks.read().count() > 0 {
        labels.push("Clicked_Default");
    }
    labels.dedup();
    for label in labels {
        match sounds.by_label.get(&label.to_ascii_lowercase()) {
            Some(snd) => match voice.play_sound(snd) {
                Some(d) => debug!("uisfx: {label} -> {snd} ({:.2}s)", d.as_secs_f32()),
                None => warn!("uisfx: {label} -> {snd} not found"),
            },
            None => warn!("uisfx: no guisounds row {label}"),
        }
    }
}

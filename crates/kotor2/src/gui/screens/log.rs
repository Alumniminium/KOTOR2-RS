//! The message log (dialog history, feedback, combat), messages_p and
//! journal_p.

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::{World, WorldEvent};

use super::{ScreenState, member};
use crate::game::{FrameEvents, WorldRes};
use crate::gui::menus::{MenuScreen, OpenMenu, open_menu};
use crate::gui::{GuiClick, GuiScreen};
use crate::model::GameRes;
use crate::uisfx::UiSfx;

const MAX_LINES: usize = 300;

#[derive(Resource, Default)]
pub struct MessageLog {
    pub dialog: Vec<String>,
    pub feedback: Vec<String>,
    pub combat: Vec<String>,
    /// Quests seen so far, to tell new ones from updates.
    known_quests: std::collections::HashSet<String>,
    /// (dialog, entry, text) of the last NPC line logged.
    last_line: Option<(String, usize, String)>,
}

fn push(v: &mut Vec<String>, s: String) {
    v.push(s);
    if v.len() > MAX_LINES {
        v.remove(0);
    }
}

fn name(w: &World, id: ObjectId) -> String {
    let n = w.get(id).map(|o| w.substitute(&o.name)).unwrap_or_default();
    if n.is_empty() { w.get(id).map(|o| o.tag.clone()).unwrap_or_default() } else { n }
}

/// guisounds.2da labels the world asks for, as the 'static names UiSfx takes.
fn static_cue(label: &str) -> Option<&'static str> {
    const KNOWN: [&str; 8] = ["Prompt", "Error_Default", "Inventory_Add", "LightSide", "DarkSide", "Clicked_Default", "Level_Up_Notify", "Quest_New_Notify"];
    KNOWN.into_iter().find(|k| k.eq_ignore_ascii_case(label))
}

const SKILLS: [&str; 8] = ["Computer Use", "Demolitions", "Stealth", "Awareness", "Persuade", "Repair", "Security", "Treat Injury"];

/// Records combat rolls, items, skill checks, journal updates and the
/// conversation lines into the log.
pub fn collect(events: Res<FrameEvents>, mut world: ResMut<WorldRes>, mut log: ResMut<MessageLog>, mut sfx: MessageWriter<UiSfx>) {
    let w = &mut world.0;
    for e in &events.0 {
        match e {
            WorldEvent::AttackHit { attacker, target, damage, critical } => {
                let crit = if *critical { " (critical)" } else { "" };
                let line = format!("{} attacks {}: hit{crit}, {damage} damage", name(w, *attacker), name(w, *target));
                push(&mut log.combat, line);
            }
            WorldEvent::AttackMissed { attacker, target } => {
                let line = format!("{} attacks {}: miss", name(w, *attacker), name(w, *target));
                push(&mut log.combat, line);
            }
            WorldEvent::Died { object, .. } => {
                let line = format!("{} is killed", name(w, *object));
                push(&mut log.combat, line);
            }
            WorldEvent::ItemReceived { name: item, stack, .. } => {
                let n = if *stack > 1 { format!(" (x{stack})") } else { String::new() };
                push(&mut log.feedback, format!("Item received: {item}{n}"));
            }
            WorldEvent::SkillCheck { user, skill, dc, total, success, .. } => {
                let s = SKILLS.get(*skill as usize).copied().unwrap_or("Skill");
                let r = if *success { "success" } else { "failure" };
                let line = format!("{} - {s}: {total} vs DC {dc}: {r}", name(w, *user));
                push(&mut log.feedback, line);
            }
            WorldEvent::Journal { plot, .. } => {
                let q = w.journal_view().into_iter().find(|q| q.tag == *plot);
                let completed = q.as_ref().is_some_and(|q| q.completed);
                let new = log.known_quests.insert(plot.clone());
                sfx.write(UiSfx(if completed {
                    "Quest_Complete_Notify"
                } else if new {
                    "Quest_New_Notify"
                } else {
                    "Prompt"
                }));
                let q = q.map(|q| q.name).unwrap_or_else(|| plot.clone());
                push(&mut log.feedback, format!("Journal updated: {q}"));
            }
            WorldEvent::XpGained { creature, amount } => {
                let line = format!("{} gains {amount} experience", name(w, *creature));
                push(&mut log.feedback, line);
            }
            WorldEvent::LevelUpAvailable { creature, level } => {
                let line = format!("{} can level up to level {level}", name(w, *creature));
                push(&mut log.feedback, line);
                sfx.write(UiSfx("Level_Up_Notify"));
            }
            WorldEvent::ConversationEnded { .. } => {
                log.last_line = None;
            }
            WorldEvent::Feedback { text, cue } => {
                push(&mut log.feedback, text.clone());
                if let Some(label) = cue.as_deref().and_then(static_cue) {
                    sfx.write(UiSfx(label));
                }
            }
            _ => {}
        }
    }
    let picked: Vec<String> = w.sheet.chosen_replies.drain(..).collect();
    if !picked.is_empty() {
        let pc = name(w, w.party_leader());
        for r in picked {
            push(&mut log.dialog, format!("{pc}: {r}"));
        }
    }
    if let Some(v) = w.conversation_view() {
        let key = (v.dialog.clone(), v.entry, v.text.clone());
        if log.last_line.as_ref() != Some(&key) && !v.text.is_empty() {
            let who = name(w, v.speaker);
            push(&mut log.dialog, format!("{who}: {}", v.text));
            log.last_line = Some(key);
        }
    }
}

pub fn messages(
    world: ResMut<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<ScreenState>,
    log: Res<MessageLog>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
) {
    if open.0.as_deref() != Some("messages") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("messages")) else { return };
    for c in clicks.read().filter(|c| c.screen.starts_with("messages")) {
        st.msg_tab = match c.tag.as_str() {
            "BTN_DIALOG" => 0,
            "BTN_FEEDBACK" => 1,
            "BTN_COMBAT" => 2,
            "BTN_EFFECTS" => 3,
            _ => st.msg_tab,
        };
    }
    let tab = st.msg_tab;
    for (t, n) in [("BTN_DIALOG", 0), ("BTN_FEEDBACK", 1), ("BTN_COMBAT", 2), ("BTN_EFFECTS", 3)] {
        scr.set_selected(t, tab == n);
    }
    scr.set_visible("LB_DIALOG", tab == 0);
    scr.set_visible("LB_MESSAGES", tab == 1);
    scr.set_visible("LB_COMBAT", tab == 2);
    for t in ["LB_EFFECTS_GOOD", "LB_EFFECTS_BAD", "LBL_EFFECTS_GOOD", "LBL_EFFECTS_BAD"] {
        scr.set_visible(t, tab == 3);
    }
    let newest_first = |v: &[String]| v.iter().rev().cloned().collect::<Vec<_>>();
    match tab {
        0 => scr.set_items("LB_DIALOG", newest_first(&log.dialog)),
        1 => scr.set_items("LB_MESSAGES", newest_first(&log.feedback)),
        2 => scr.set_items("LB_COMBAT", newest_first(&log.combat)),
        _ => {
            let w = &world.0;
            let me = member(w, &st);
            let fx = w.effect_views(me);
            scr.set_items("LB_EFFECTS_GOOD", fx.iter().filter(|(_, g)| *g).map(|(n, _)| n.clone()).collect());
            scr.set_items("LB_EFFECTS_BAD", fx.iter().filter(|(_, g)| !*g).map(|(n, _)| n.clone()).collect());
        }
    }
}

#[allow(clippy::too_many_arguments)]
pub fn journal(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut open: ResMut<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    if open.0.as_deref() != Some("journal") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("journal")) else { return };
    let mut to_messages = false;
    for c in clicks.read().filter(|c| c.screen.starts_with("journal")) {
        match c.tag.as_str() {
            "BTN_SWAPTEXT" => {
                st.journal_completed = !st.journal_completed;
                scr.set_list_selected("LB_ITEMS", None);
            }
            "BTN_FILTER_TIME" => st.journal_sort = 0,
            "BTN_FILTER_NAME" => st.journal_sort = 1,
            "BTN_FILTER_PRIORITY" => st.journal_sort = 2,
            "BTN_FILTER_PLANET" => st.journal_sort = 3,
            "BTN_MESSAGES" => to_messages = true,
            _ => {}
        }
    }
    if to_messages {
        open_menu(&mut commands, &game, &mut open, &existing, "messages");
        return;
    }
    let mut quests: Vec<_> = world.0.journal_view().into_iter().filter(|q| q.completed == st.journal_completed).collect();
    match st.journal_sort {
        1 => quests.sort_by(|a, b| a.name.cmp(&b.name)),
        2 => quests.sort_by_key(|q| q.priority),
        3 => quests.sort_by_key(|q| q.planet),
        _ => {}
    }
    for (t, n) in [("BTN_FILTER_TIME", 0), ("BTN_FILTER_NAME", 1), ("BTN_FILTER_PRIORITY", 2), ("BTN_FILTER_PLANET", 3)] {
        scr.set_selected(t, st.journal_sort == n);
    }
    scr.set_text("BTN_SWAPTEXT", if st.journal_completed { "Completed Quests" } else { "Active Quests" });
    scr.set_items("LB_ITEMS", quests.iter().map(|q| q.name.clone()).collect());
    if !quests.is_empty() && scr.list_selected("LB_ITEMS").is_none() {
        scr.set_list_selected("LB_ITEMS", Some(0));
    }
    let shown = scr.hovered_item("LB_ITEMS").or(scr.list_selected("LB_ITEMS")).and_then(|i| quests.get(i));
    let text = match shown {
        Some(q) => format!("{}\n\n{}", q.name, q.text),
        None if st.journal_completed => "No completed quests.".into(),
        None => "No active quests.".into(),
    };
    scr.set_text("LBL_ITEM_DESCRIPTION", text);
}

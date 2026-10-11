//! character_p and abilities_p.

use bevy::prelude::*;
use k2_world::sheet::ability_mod;

use super::{ScreenState, member};
use crate::game::WorldRes;
use crate::gui::menus::{MenuScreen, OpenMenu};
use crate::gui::{GuiClick, GuiScreen, ListItem};

const ABILITY_TAGS: [(&str, &str); 6] = [
    ("LBL_STR", "LBL_STR_MOD"),
    ("LBL_DEX", "LBL_DEX_MOD"),
    ("LBL_CON", "LBL_CON_MOD"),
    ("LBL_INT", "LBL_INT_MOD"),
    ("LBL_WIS", "LBL_WIS_MOD"),
    ("LBL_CHA", "LBL_CHA_MOD"),
];

fn thousands(n: i32) -> String {
    let s = n.abs().to_string();
    let mut out = String::new();
    for (i, ch) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    if n < 0 { format!("-{out}") } else { out }
}

pub fn character(world: ResMut<WorldRes>, open: Res<OpenMenu>, st: Res<ScreenState>, mut screens: Query<&mut GuiScreen, With<MenuScreen>>) {
    if open.0.as_deref() != Some("character") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("character")) else { return };
    let mut world = world;
    let w = &mut world.0;
    let me = member(w, &st);
    let Some(sh) = w.character_sheet(me) else { return };
    for (i, (score, m)) in ABILITY_TAGS.iter().enumerate() {
        scr.set_text(score, sh.abilities[i].to_string());
        scr.set_text(m, format!("{:+}", ability_mod(sh.abilities[i])));
    }
    scr.set_text("LBL_VITALITY_STAT", format!("{}/{}", sh.hp, sh.max_hp));
    scr.set_text("LBL_FORCE_STAT", format!("{}/{}", sh.force, sh.max_force));
    scr.set_text("LBL_DEFENSE_STAT", sh.defense.to_string());
    scr.set_text("LBL_FORTITUDE_STAT", format!("{:+}", sh.fortitude));
    scr.set_text("LBL_REFLEX_STAT", format!("{:+}", sh.reflex));
    scr.set_text("LBL_WILL_STAT", format!("{:+}", sh.will));
    scr.set_text("LBL_EXPERIENCE_STAT", thousands(sh.xp));
    scr.set_text("LBL_NEEDED_XP", thousands(sh.next_level_xp));
    scr.set_text("LBL_FORCEMASTERY", "");
    // Alignment slider: good_evil 0 (dark) .. 100 (light); show it as the
    // slider's progress-style thumb position is not modelled, so hide it.
    scr.set_visible("SLD_ALIGN", false);
    scr.set_visible("BTN_LEVELUP", false);
    scr.set_visible("BTN_AUTO", false);
    let classes = sh.classes.iter().map(|(n, l)| format!("{n} {l}")).collect::<Vec<_>>().join(" / ");
    scr.set_text("LBL_3DCHAR", format!("{}\n{}", sh.name, classes));
}

pub fn abilities(
    mut world: ResMut<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
) {
    if open.0.as_deref() != Some("abilities") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("abilities")) else { return };
    for c in clicks.read().filter(|c| c.screen.starts_with("abilities")) {
        let tab = match c.tag.as_str() {
            "BTN_SKILLS" => 0,
            "BTN_POWERS" => 1,
            "BTN_FEATS" => 2,
            _ => continue,
        };
        if st.abil_tab != tab {
            st.abil_tab = tab;
            scr.set_list_selected("LB_ABILITY", None);
        }
    }
    scr.set_selected("BTN_SKILLS", st.abil_tab == 0);
    scr.set_selected("BTN_POWERS", st.abil_tab == 1);
    scr.set_selected("BTN_FEATS", st.abil_tab == 2);
    let w = &mut world.0;
    let me = member(w, &st);
    // (row text, name, description, rank, bonus) per tab.
    let entries: Vec<(String, String, Option<(i32, i32)>, Option<String>)> = match st.abil_tab {
        0 => w.skill_views(me).into_iter().map(|s| (s.name, s.description, Some((s.rank, s.bonus)), None)).collect(),
        2 => w.feat_views(me).into_iter().map(|f| (f.name, f.description, None, f.icon)).collect(),
        _ => w.power_views(me).into_iter().map(|f| (f.name, f.description, None, f.icon)).collect(),
    };
    scr.set_list("LB_ABILITY", entries.iter().map(|(n, _, _, icon)| ListItem::text(n.clone()).with_icon(icon.clone())).collect());
    if !entries.is_empty() && scr.list_selected("LB_ABILITY").is_none() {
        scr.set_list_selected("LB_ABILITY", Some(0));
    }
    let shown = scr.hovered_item("LB_ABILITY").or(scr.list_selected("LB_ABILITY")).and_then(|i| entries.get(i));
    let skills = st.abil_tab == 0;
    scr.set_text("LBL_SKILLRANK", "Skill Rank");
    scr.set_text("LBL_BONUS", "Ability Modifier");
    scr.set_text("LBL_TOTAL", "Total");
    for t in ["LBL_SKILLRANK", "LBL_RANKVAL", "LBL_BONUS", "LBL_BONUSVAL", "LBL_TOTAL", "LBL_TOTALVAL", "LBL_INFOBG", "LB_DESC"] {
        scr.set_visible(t, skills);
    }
    scr.set_visible("LB_DESC_FEATS", !skills);
    let empty = match st.abil_tab {
        1 => "No Force powers.",
        2 => "No feats.",
        _ => "",
    };
    match shown {
        Some((name, desc, ranks, _)) => {
            scr.set_text("LBL_NAME", name.clone());
            scr.set_text(if skills { "LB_DESC" } else { "LB_DESC_FEATS" }, desc.clone());
            if let Some((rank, bonus)) = ranks {
                scr.set_text("LBL_RANKVAL", format!("{rank:+}"));
                scr.set_text("LBL_BONUSVAL", format!("{bonus:+}"));
                scr.set_text("LBL_TOTALVAL", format!("{:+}", rank + bonus));
            }
        }
        None => {
            scr.set_text("LBL_NAME", "");
            scr.set_text("LB_DESC_FEATS", empty);
        }
    }
}

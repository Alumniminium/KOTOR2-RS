//! inventory_p and equip_p.

use bevy::prelude::*;
use k2_world::inventory::{ItemView, SLOT_NAMES};

use super::{Filter, ScreenState, classify, describe, item_row, member, passes};
use crate::game::WorldRes;
use crate::gui::menus::{MenuScreen, OpenMenu};
use crate::gui::{GuiClick, GuiScreen, ListItem};
use crate::uisfx::UiSfx;

const INV_FILTERS: &[(&str, Filter)] = &[
    ("BTN_ALL", Filter::All),
    ("BTN_DATAPADS", Filter::Datapads),
    ("BTN_WEAPONS", Filter::Weapons),
    ("BTN_ARMOR", Filter::Armor),
    ("BTN_USEABLE", Filter::Useable),
    ("BTN_QUESTS", Filter::Quests),
    ("BTN_MISC", Filter::Misc),
];

/// The party inventory under the current filter.
fn filtered(w: &mut k2_world::World, f: Filter) -> Vec<ItemView> {
    let all = w.party_inventory_views();
    all.into_iter().filter(|v| passes(f, classify(w, v))).collect()
}

pub fn inventory(
    mut world: ResMut<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    mut sfx: MessageWriter<UiSfx>,
) {
    if open.0.as_deref() != Some("inventory") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("inventory")) else { return };
    let w = &mut world.0;
    let me = member(w, &st);
    for c in clicks.read().filter(|c| c.screen.starts_with("inventory")) {
        if let Some(&(_, f)) = INV_FILTERS.iter().find(|(t, _)| *t == c.tag) {
            st.inv_filter = f;
            scr.set_list_selected("LB_ITEMS", None);
            sfx.write(UiSfx("Checkbox_Check"));
        } else if c.tag == "BTN_USEITEM" {
            let items = filtered(w, st.inv_filter);
            if let Some(v) = scr.list_selected("LB_ITEMS").and_then(|i| items.get(i)) {
                st.status = Some(match w.use_item(me, v.id, me) {
                    Ok(_) => {
                        sfx.write(UiSfx("Inventory_Select"));
                        format!("Used {}.", v.name)
                    }
                    Err(e) => {
                        sfx.write(UiSfx("Error_Default"));
                        format!("{}: {e}", v.name)
                    }
                });
            }
        } else if c.tag == "LB_ITEMS" && c.item.is_some() {
            sfx.write(UiSfx("Inventory_Select"));
        }
    }
    for &(tag, f) in INV_FILTERS {
        scr.set_selected(tag, st.inv_filter == f);
    }
    let items = filtered(w, st.inv_filter);
    scr.set_text("LBL_CREDITS_VALUE", w.credits().to_string());
    scr.set_list("LB_ITEMS", items.iter().map(item_row).collect());
    if !items.is_empty() && scr.list_selected("LB_ITEMS").is_none() {
        scr.set_list_selected("LB_ITEMS", Some(0));
    }
    let sel = scr.list_selected("LB_ITEMS").and_then(|i| items.get(i));
    let usable = sel.is_some_and(|v| w.is_usable_item(v.id));
    scr.set_enabled("BTN_USEITEM", usable);
    let shown = scr.hovered_item("LB_ITEMS").and_then(|i| items.get(i)).or(sel);
    let mut desc = shown.map(describe).unwrap_or_default();
    if let Some(s) = &st.status {
        desc = format!("{s}\n\n{desc}");
    }
    scr.set_text("LB_DESCRIPTION", desc);
}

/// equip_p slot buttons and their INVENTORY_SLOT_*. The weapon buttons use
/// stable logical slots: 4/5 are the hands of weapon set 1 and 18/19 those
/// of set 2, wherever the world keeps them right now (`physical`).
const SLOTS: &[(&str, i32)] = &[
    ("BTN_INV_HEAD", 0),
    ("BTN_INV_BODY", 1),
    ("BTN_INV_HANDS", 3),
    ("BTN_INV_WEAP_R", 4),
    ("BTN_INV_WEAP_L", 5),
    ("BTN_INV_WEAP_R2", 18),
    ("BTN_INV_WEAP_L2", 19),
    ("BTN_INV_ARM_L", 7),
    ("BTN_INV_ARM_R", 8),
    ("BTN_INV_IMPLANT", 9),
    ("BTN_INV_BELT", 10),
];

/// The world slot behind an equip_p logical slot (swapping sets moves the
/// live pair between slots 4/5 and 18/19).
fn physical(w: &k2_world::World, me: k2_script::ObjectId, logical: i32) -> i32 {
    match logical {
        4 => w.weapon_set_slots(me, 0).0,
        5 => w.weapon_set_slots(me, 0).1,
        18 => w.weapon_set_slots(me, 1).0,
        19 => w.weapon_set_slots(me, 1).1,
        s => s,
    }
}

/// Inverse of `physical`.
fn logical(w: &k2_world::World, me: k2_script::ObjectId, phys: i32) -> i32 {
    for (set, base) in [(0, 4), (1, 18)] {
        let (right, left) = w.weapon_set_slots(me, set);
        if phys == right {
            return base;
        }
        if phys == left {
            return base + 1;
        }
    }
    phys
}

/// Seconds a hovered slot keeps its candidate list after the mouse leaves
/// it, so the pointer can cross the gap to the list and click a row.
const HOVER_GRACE: f32 = 0.8;

/// The slot being shown: the hovered one, else the clicked one.
fn shown_slot(st: &ScreenState) -> Option<i32> {
    st.equip_hover.map(|(s, _)| s).or(st.equip_slot)
}

/// Test hook: `K2_GUI_HOVER=BTN_INV_WEAP_R` acts as if the mouse sat on
/// that slot button.
fn forced_hover() -> Option<i32> {
    let tag = std::env::var("K2_GUI_HOVER").ok()?;
    SLOTS.iter().find(|(t, _)| *t == tag).map(|&(_, s)| s)
}

pub fn equip(
    mut world: ResMut<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    mut sfx: MessageWriter<UiSfx>,
    time: Res<Time>,
    mut forced: Local<Option<Option<i32>>>,
) {
    if open.0.as_deref() != Some("equip") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("equip")) else { return };
    let w = &mut world.0;
    let me = member(w, &st);
    // Hovering a slot previews what fits it; the preview survives the trip
    // from the slot to the list (over the list, or for a short grace).
    let before = shown_slot(&st);
    let over_slot = (*forced.get_or_insert_with(forced_hover)).or_else(|| SLOTS.iter().find(|(t, _)| scr.is_hovered(t)).map(|&(_, s)| s));
    let over_list = scr.is_hovered("LB_ITEMS") || scr.hovered_item("LB_ITEMS").is_some();
    let dt = time.delta_secs();
    st.equip_hover = match (over_slot, st.equip_hover) {
        (Some(s), _) => Some((s, 0.0)),
        (None, Some((s, _))) if over_list => Some((s, 0.0)),
        (None, Some((s, t))) if t + dt < HOVER_GRACE => Some((s, t + dt)),
        _ => None,
    };
    if shown_slot(&st) != before {
        scr.set_list_selected("LB_ITEMS", None);
    }
    for c in clicks.read().filter(|c| c.screen.starts_with("equip")) {
        if let Some(&(_, slot)) = SLOTS.iter().find(|(t, _)| *t == c.tag) {
            st.equip_slot = if st.equip_slot == Some(slot) { None } else { Some(slot) };
            sfx.write(UiSfx("Inventory_Select"));
            st.status = None;
            scr.set_list_selected("LB_ITEMS", None);
            continue;
        }
        match c.tag.as_str() {
            "BTN_SWAPWEAPONS" => {
                let swapped = w.swap_weapon_sets(me);
                sfx.write(UiSfx(if swapped { "Inventory_Select" } else { "Error_Default" }));
                st.status = None;
                scr.set_list_selected("LB_ITEMS", None);
            }
            "BTN_EQUIP" | "LB_ITEMS" => {
                // Row 0 is "unequip" when the slot holds something.
                let Some(slot) = shown_slot(&st) else { continue };
                let Some(row) = scr.list_selected("LB_ITEMS") else { continue };
                if c.tag == "LB_ITEMS" && c.item.is_none() {
                    continue;
                }
                let slot = physical(w, me, slot);
                let equipped = w.equipment(me).into_iter().find(|&(s, _)| s == slot).map(|(_, i)| i);
                let offset = usize::from(equipped.is_some());
                let unequip = equipped.is_some() && row == 0;
                let r = if unequip {
                    w.unequip(me, slot).map(|_| ()).ok_or_else(|| "nothing to remove".to_string())
                } else {
                    let list = w.equip_candidates(me, slot);
                    match list.get(row - offset) {
                        Some(v) => w.equip(me, v.id, slot).map_err(|e| e.to_string()),
                        None => Ok(()),
                    }
                };
                sfx.write(UiSfx(match (&r, unequip) {
                    (Err(_), _) => "Error_Default",
                    (Ok(()), true) => "Inventory_Drop",
                    (Ok(()), false) => "Inventory_Add",
                }));
                if c.tag == "BTN_EQUIP" || r.is_err() {
                    st.status = r.err();
                }
                if st.status.is_none() {
                    st.equip_slot = None;
                    st.equip_hover = None;
                    scr.set_list_selected("LB_ITEMS", None);
                }
            }
            _ => {}
        }
    }
    let equipment = w.equipment(me);
    let views: Vec<(i32, ItemView)> = equipment.iter().filter_map(|&(s, i)| w.item_view(i).map(|v| (logical(w, me, s), v))).collect();
    for &(tag, slot) in SLOTS {
        let icon = views.iter().find(|(s, _)| *s == slot).and_then(|(_, v)| v.icon.clone());
        scr.set_fill(tag, icon);
        scr.set_selected(tag, st.equip_slot == Some(slot));
    }
    for t in ["BTN_PREVNPC", "BTN_NEXTNPC"] {
        scr.set_visible(t, false);
    }
    scr.set_visible("LBL_CANTEQUIP", false);
    scr.set_text("BTN_SWAPWEAPONS", format!("Switch Weapons (Set {})", w.weapon_set(me) + 1));
    // Stats.
    if let Some(sh) = w.character_sheet(me) {
        scr.set_text("LBL_DEF", sh.defense.to_string());
        scr.set_text("LBL_TOHITR", format!("{:+}", sh.attack_main));
        scr.set_text("LBL_ATKR", sh.damage_main.clone());
        scr.set_text("LBL_TOHITL", sh.attack_off.map(|a| format!("{a:+}")).unwrap_or_default());
        scr.set_text("LBL_ATKL", sh.damage_off.clone().unwrap_or_default());
    }
    let selected_slot = shown_slot(&st);
    let slot_name = selected_slot
        .map(|s| match s {
            4 | 5 | 18 | 19 => format!("{} (set {})", capitalize(SLOT_NAMES[if s >= 18 { s - 14 } else { s } as usize]), if s < 18 { 1 } else { 2 }),
            s => SLOT_NAMES.get(s as usize).map(|n| capitalize(n)).unwrap_or_default(),
        })
        .unwrap_or_default();
    scr.set_text("LBL_SLOTNAME", slot_name);
    let (rows, list): (Vec<ListItem>, Vec<Option<ItemView>>) = match selected_slot {
        None => {
            // No slot picked: list what is equipped.
            let mut v: Vec<(i32, ItemView)> = views.clone();
            v.sort_by_key(|(s, _)| *s);
            (v.iter().map(|(_, it)| item_row(it)).collect(), v.into_iter().map(|(_, it)| Some(it)).collect())
        }
        Some(slot) => {
            let cur = views.iter().find(|(s, _)| *s == slot).map(|(_, v)| v.clone());
            let mut rows = Vec::new();
            let mut list = Vec::new();
            if let Some(c) = &cur {
                rows.push(ListItem::text(format!("Unequip {}", c.name)).with_icon(c.icon.clone()));
                list.push(Some(c.clone()));
            }
            for v in w.equip_candidates(me, physical(w, me, slot)) {
                rows.push(item_row(&v));
                list.push(Some(v));
            }
            (rows, list)
        }
    };
    scr.set_list("LB_ITEMS", rows);
    let any = scr.list_selected("LB_ITEMS").is_some();
    scr.set_enabled("BTN_EQUIP", selected_slot.is_some() && any);
    // The description pane shares the right half with the slot grid: it is
    // shown while a row is hovered (or a status message is up).
    let shown = scr.hovered_item("LB_ITEMS").and_then(|i| list.get(i).cloned().flatten());
    let desc_mode = shown.is_some() || st.status.is_some();
    let mut desc = shown.as_ref().map(describe).unwrap_or_default();
    if let Some(s) = &st.status {
        desc = format!("{s}\n\n{desc}");
    }
    scr.set_text("LB_DESC", desc);
    scr.set_visible("LB_DESC", desc_mode);
    let grid: Vec<String> = scr
        .def
        .controls
        .iter()
        .map(|c| c.tag.clone())
        .filter(|t| {
            (t.starts_with("BTN_INV_") || t.starts_with("LBL_INV_") || t.starts_with("LBL_DEF") || t.starts_with("LBL_TOHIT") || t.starts_with("LBL_ATK"))
                || ["LBL_DAMAGE", "LBL_ATTACKMOD", "LBL_DAMTEXT", "LBL_BACK1", "BTN_SWAPWEAPONS"].contains(&t.as_str())
        })
        .collect();
    for t in grid {
        scr.set_visible(&t, !desc_mode);
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

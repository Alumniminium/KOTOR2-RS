//! Workbench and lab station: upgradesel_p (pick an item to upgrade),
//! upgrade_p (its slots and the upgrades that fit), component_p /
//! chemical_p (create items from components / chemicals, or break items
//! down into them).

use bevy::prelude::*;
use k2_script::ObjectId;
use k2_world::World;
use k2_world::craft::{Currency, Station, UpgradeSlot};
use k2_world::inventory::ItemView;

use super::{ScreenState, describe, item_row, party};
use crate::game::WorldRes;
use crate::gui::menus::{MenuScreen, OpenMenu, close_menu, open_menu};
use crate::gui::{GuiClick, GuiScreen, ListItem};
use crate::model::GameRes;
use crate::uisfx::UiSfx;

/// 0 ranged, 1 melee, 2 armour, 3 lightsaber, from the first slot's type.
fn kind_of(slots: &[UpgradeSlot]) -> i32 {
    match slots.first().map_or(-2, |s| s.ty) {
        -1..=3 => 3,
        4..=6 => 0,
        7..=9 => 1,
        _ => 2,
    }
}

/// Items the party could upgrade: inventory plus everyone's equipment.
fn upgradable(w: &mut World) -> Vec<(ItemView, i32)> {
    let mut ids = w.party_inventory();
    for m in party(w) {
        ids.extend(w.equipment(m).into_iter().map(|(_, i)| i));
    }
    let mut out = Vec::new();
    for id in ids {
        let slots = w.upgrade_slots(id);
        if slots.is_empty() {
            continue;
        }
        if let Some(v) = w.item_view(id) {
            out.push((v, kind_of(&slots)));
        }
    }
    out
}

const SEL_FILTERS: &[(&str, i32)] = &[("BTN_ALL", -1), ("BTN_RANGED", 0), ("BTN_MELEE", 1), ("BTN_ARMOR", 2), ("BTN_LIGHTSABER", 3)];

#[allow(clippy::too_many_arguments)]
pub fn upgrade_select(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut open: ResMut<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    existing: Query<Entity, With<MenuScreen>>,
) {
    if open.0.as_deref() != Some("upgradesel") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("upgradesel")) else { return };
    let Some(ui) = st.craft.as_mut() else { return };
    let w = &mut world.0;
    let items: Vec<(ItemView, i32)> = upgradable(w).into_iter().filter(|(_, k)| ui.kind < 0 || *k == ui.kind).collect();
    // upgradesel_p has no create button: when the bench also allows item
    // creation, the list's first row leads to the create / break down
    // screen.
    let create_row = usize::from(ui.item_creation);
    let mut next: Option<&str> = None;
    let create_screen = if ui.station == Station::LabStation { "chemical" } else { "component" };
    for c in clicks.read().filter(|c| c.screen.starts_with("upgradesel")) {
        if let Some(&(_, k)) = SEL_FILTERS.iter().find(|(t, _)| *t == c.tag) {
            ui.kind = k;
            scr.set_list_selected("LB_UPGRADELIST", None);
            continue;
        }
        match c.tag.as_str() {
            "BTN_UPGRADEITEMS" => match scr.list_selected("LB_UPGRADELIST") {
                Some(0) if create_row == 1 => next = Some(create_screen),
                Some(i) => {
                    if let Some((v, _)) = items.get(i - create_row) {
                        ui.item = Some(v.id);
                        ui.slot = None;
                        next = Some("upgrade");
                    }
                }
                None => {}
            },
            "BTN_BACK" => next = Some("close"),
            _ => {}
        }
    }
    for &(t, k) in SEL_FILTERS {
        scr.set_selected(t, ui.kind == k);
    }
    let mut rows: Vec<ListItem> = Vec::new();
    if create_row == 1 {
        rows.push(ListItem::text(if ui.station == Station::LabStation { "Create Items" } else { "Create / Break Down Items" }));
    }
    rows.extend(items.iter().map(|(v, _)| item_row(v)));
    let n = rows.len();
    scr.set_list("LB_UPGRADELIST", rows);
    if n > 0 && scr.list_selected("LB_UPGRADELIST").is_none() {
        scr.set_list_selected("LB_UPGRADELIST", Some(0));
    }
    let sel = scr.list_selected("LB_UPGRADELIST");
    scr.set_enabled("BTN_UPGRADEITEMS", sel.is_some());
    scr.set_text("BTN_UPGRADEITEMS", if create_row == 1 && sel == Some(0) { "Open" } else { "Upgrade Item" });
    let row = scr.hovered_item("LB_UPGRADELIST").or(sel);
    let text = match row {
        Some(0) if create_row == 1 => "Create new items from components, or break items down into components.".to_owned(),
        Some(i) => items.get(i - create_row).map(|(v, _)| describe(v)).unwrap_or_default(),
        None => "No items that can be upgraded.".into(),
    };
    scr.set_text("LB_DESCRIPTION", text);
    match next {
        Some("close") => close_menu(&mut commands, &mut open, &existing),
        Some(base) => open_menu(&mut commands, &game, &mut open, &existing, base),
        None => {}
    }
}

const SLOT_TAGS: [&str; 3] = ["31", "32", "33"];
const SABER_TAGS: [&str; 6] = ["31", "32", "33", "34", "35", "36"];

#[allow(clippy::too_many_arguments)]
pub fn upgrade(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut open: ResMut<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    existing: Query<Entity, With<MenuScreen>>,
    mut sfx: MessageWriter<UiSfx>,
) {
    if open.0.as_deref() != Some("upgrade") {
        return;
    }
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("upgrade") && !s.name.starts_with("upgradesel")) else { return };
    let Some(ui) = st.craft.as_mut() else { return };
    let w = &mut world.0;
    let Some(mut item) = ui.item else { return };
    let mut slots = w.upgrade_slots(item);
    let saber = kind_of(&slots) == 3;
    let (tags, suffix) = if saber { (&SABER_TAGS[..], "_LS") } else { (&SLOT_TAGS[..], "") };
    let mut back = false;
    for c in clicks.read().filter(|c| c.screen.starts_with("upgrade") && !c.screen.starts_with("upgradesel")) {
        if let Some(n) = tags.iter().position(|t| c.tag == format!("BTN_UPGRADE{t}{suffix}")) {
            ui.slot = if n < slots.len() { Some(n) } else { None };
            scr.set_list_selected("LB_ITEMS", None);
            continue;
        }
        match (c.tag.as_str(), c.item) {
            ("LB_ITEMS", Some(row)) => {
                let Some(s) = ui.slot.and_then(|n| slots.get(n)) else { continue };
                let offset = usize::from(s.installed.is_some());
                let removing = s.installed.is_some() && row == 0;
                let r = if removing {
                    w.remove_upgrade(item, s.slot).map(|_| item)
                } else {
                    match s.candidates.get(row - offset) {
                        Some(&u) => w.apply_upgrade(item, s.slot, u),
                        None => Ok(item),
                    }
                };
                sfx.write(UiSfx(match (&r, removing) {
                    (Err(_), _) => "Error_Default",
                    (Ok(_), true) => "Inventory_Drop",
                    (Ok(_), false) => "Upgrade",
                }));
                match r {
                    Ok(it) => {
                        item = it;
                        ui.item = Some(it);
                        ui.status = String::new();
                        ui.slot = None;
                    }
                    Err(e) => ui.status = e.to_string(),
                }
                scr.set_list_selected("LB_ITEMS", None);
                slots = w.upgrade_slots(item);
            }
            // Back closes the upgrade list first, then the screen.
            ("BTN_BACK", _) if ui.slot.is_some() => ui.slot = None,
            ("BTN_BACK" | "BTN_OK" | "BTN_ASSEMBLE", _) => back = true,
            _ => {}
        }
    }
    let Some(view) = w.item_view(item) else { return };
    scr.set_text("LBL_TITLE", view.name.clone());
    scr.set_visible("3D_MODEL", false);
    scr.set_visible("3D_MODEL_LS", false);
    scr.set_visible("BTN_OK", false);
    // Slot buttons of the other layout (saber vs not) stay hidden.
    for t in SABER_TAGS {
        for p in ["LBL_UPGRADE", "BTN_UPGRADE"] {
            scr.set_visible(&format!("{p}{t}_LS"), false);
        }
    }
    for t in SLOT_TAGS {
        for p in ["LBL_UPGRADE", "BTN_UPGRADE"] {
            scr.set_visible(&format!("{p}{t}"), false);
        }
    }
    // The upgrade list covers the slot area while a slot is open.
    let listing = ui.slot.is_some();
    scr.set_visible("LB_ITEMS", listing);
    for (n, t) in tags.iter().enumerate() {
        let s = slots.get(n);
        let (lbl, btn) = (format!("LBL_UPGRADE{t}{suffix}"), format!("BTN_UPGRADE{t}{suffix}"));
        scr.set_visible(&lbl, s.is_some() && !listing);
        scr.set_visible(&btn, s.is_some() && !listing);
        let icon = s.and_then(|s| s.installed.clone()).and_then(|tpl| w.item_def(&tpl)).and_then(|d| d.icon.clone());
        scr.set_fill(&btn, icon);
        scr.set_selected(&btn, ui.slot == Some(n));
    }
    let sel = ui.slot.and_then(|n| slots.get(n));
    scr.set_text("LBL_SLOTNAME", sel.map(|s| s.label.clone()).unwrap_or_else(|| "Select an upgrade slot".into()));
    let mut rows = Vec::new();
    let mut views: Vec<Option<ItemView>> = Vec::new();
    if let Some(s) = sel {
        if let Some(tpl) = &s.installed {
            let d = w.item_def(tpl);
            rows.push(ListItem::text(format!("Remove {}", d.as_ref().map_or(tpl.clone(), |d| d.name.clone()))).with_icon(d.and_then(|d| d.icon.clone())));
            views.push(None);
        }
        for &u in &s.candidates {
            if let Some(v) = w.item_view(u) {
                rows.push(item_row(&v));
                views.push(Some(v));
            }
        }
    }
    if sel.is_some() && rows.is_empty() {
        rows.push(ListItem { enabled: false, ..ListItem::text("No upgrades for this slot") });
        views.push(None);
    }
    scr.set_list("LB_ITEMS", rows);
    let hovered = scr.hovered_item("LB_ITEMS").and_then(|i| views.get(i).cloned().flatten());
    let installed: Vec<String> = slots
        .iter()
        .map(|s| format!("{}: {}", s.label, s.installed.as_ref().and_then(|t| w.item_def(t)).map_or("(empty)".to_owned(), |d| d.name.clone())))
        .collect();
    let mut desc = match hovered {
        Some(v) => describe(&v),
        None => format!("{}\n\n{}", describe(&view), installed.join("\n")),
    };
    if !ui.status.is_empty() {
        desc = format!("{}\n\n{desc}", ui.status);
    }
    scr.set_text("LB_DESC", desc);
    scr.set_text("LBL_PROPERTY", "");
    if back {
        ui.slot = None;
        if ui.upgrade {
            open_menu(&mut commands, &game, &mut open, &existing, "upgradesel");
        } else {
            close_menu(&mut commands, &mut open, &existing);
        }
    }
}

const CREATE_TABS_WORKBENCH: &[(&str, i32)] =
    &[("BTN_CREATE_ARMOR", 0), ("BTN_CREATE_RANGED", 1), ("BTN_CREATE_MELEE", 2), ("BTN_CREATE_LIGHTSABER", 3), ("BTN_CREATE_MISC", 4)];
const CREATE_TABS_LAB: &[(&str, i32)] = &[("BTN_CREATE_HEALTH", 0), ("BTN_CREATE_STIMS", 1), ("BTN_CREATE_MINES", 2), ("BTN_CREATE_GRENADES", 3)];
/// Breakdown filters (workbench only): -1 all, 0 weapons, 1 armour,
/// 2 useable, 3 misc.
const BREAK_TABS: &[(&str, i32)] = &[("BTN_BREAK_ALL", -1), ("BTN_BREAK_WEAPONS", 0), ("BTN_BREAK_ARMOR", 1), ("BTN_BREAK_USEABLE", 2), ("BTN_BREAK_MISC", 3)];

const SKILL_NAMES: [&str; 8] = ["Computer Use", "Demolitions", "Stealth", "Awareness", "Persuade", "Repair", "Security", "Treat Injury"];

fn break_class(w: &mut World, v: &ItemView) -> i32 {
    match super::classify(w, v) {
        super::Filter::Weapons => 0,
        super::Filter::Armor => 1,
        super::Filter::Useable => 2,
        _ => 3,
    }
}

#[allow(clippy::too_many_arguments)]
pub fn create(
    mut commands: Commands,
    game: Res<GameRes>,
    mut world: ResMut<WorldRes>,
    mut open: ResMut<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    existing: Query<Entity, With<MenuScreen>>,
    mut sfx: MessageWriter<UiSfx>,
) {
    let base = match open.0.as_deref() {
        Some(b @ ("component" | "chemical")) => b,
        _ => return,
    };
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with(base)) else { return };
    let Some(ui) = st.craft.as_mut() else { return };
    let w = &mut world.0;
    let lab = base == "chemical";
    let station = if lab { Station::LabStation } else { Station::Workbench };
    let table = if ui.recipes.is_empty() || (lab && !ui.recipes.contains("chemical")) {
        if lab { "chemicalcreate".to_owned() } else { "itemcreate".to_owned() }
    } else {
        ui.recipes.clone()
    };
    let character = if w.is_valid(ui.character) { ui.character } else { w.party_leader() };
    let tabs = if lab { CREATE_TABS_LAB } else { CREATE_TABS_WORKBENCH };
    let recipes: Vec<_> = w.recipes(&table, character).into_iter().filter(|r| r.group == ui.group).collect();
    let breakable: Vec<(ItemView, Currency, i32)> = if ui.creating {
        Vec::new()
    } else {
        let inv = w.party_inventory_views();
        let mut out = Vec::new();
        for v in inv {
            if let Ok((c, n)) = w.breakdown_preview(v.id, station, character)
                && (ui.group < 0 || break_class(w, &v) == ui.group)
            {
                out.push((v, c, n));
            }
        }
        out
    };
    let mut next: Option<&str> = None;
    for c in clicks.read().filter(|c| c.screen.starts_with(base)) {
        if let Some(&(_, g)) = tabs.iter().find(|(t, _)| *t == c.tag) {
            ui.creating = true;
            ui.group = g;
            scr.set_list_selected("LB_SHOPITEMS", None);
            continue;
        }
        if let Some(&(_, g)) = BREAK_TABS.iter().find(|(t, _)| *t == c.tag) {
            ui.creating = false;
            ui.group = g;
            scr.set_list_selected("LB_INVITEMS", None);
            continue;
        }
        match c.tag.as_str() {
            "BTN_Examine" if !lab => {
                ui.creating = !ui.creating;
                ui.group = if ui.creating { 0 } else { -1 };
                ui.status.clear();
            }
            "BTN_Accept" => {
                if ui.creating {
                    if let Some(r) = scr.list_selected("LB_SHOPITEMS").and_then(|i| recipes.get(i)) {
                        ui.status = match w.create_item_from_recipe(&table, r.row, character) {
                            Ok(_) => {
                                sfx.write(UiSfx("Inventory_Add"));
                                format!("Created {}.", r.name)
                            }
                            Err(e) => {
                                sfx.write(UiSfx("Error_Default"));
                                format!("{}: {e}", r.name)
                            }
                        };
                    }
                } else if let Some((v, _, _)) = scr.list_selected("LB_INVITEMS").and_then(|i| breakable.get(i)) {
                    ui.status = match w.break_down(v.id, station, character) {
                        Ok((cur, n)) => {
                            sfx.write(UiSfx("Inventory_Drop"));
                            format!("Broke down {} into {n} {}.", v.name, if cur == Currency::Chemicals { "chemicals" } else { "components" })
                        }
                        Err(e) => {
                            sfx.write(UiSfx("Error_Default"));
                            format!("{}: {e}", v.name)
                        }
                    };
                }
            }
            "BTN_Cancel" => next = Some(if ui.upgrade { "upgradesel" } else { "close" }),
            _ => {}
        }
    }
    let creating = ui.creating;
    for &(t, g) in tabs {
        scr.set_visible(t, creating);
        scr.set_selected(t, creating && ui.group == g);
    }
    for &(t, g) in BREAK_TABS {
        scr.set_visible(t, !creating && !lab);
        scr.set_selected(t, !creating && ui.group == g);
    }
    scr.set_visible("LB_SHOPITEMS", creating);
    scr.set_visible("LB_INVITEMS", !creating);
    scr.set_visible("LB_DESCRIPTIONINV", false);
    scr.set_visible("BTN_Examine", !lab);
    scr.set_text("BTN_Examine", if creating { "View Breakdown Items" } else { "View Create Items" });
    scr.set_text("BTN_Accept", if creating { "Create Item" } else { "Break Down Item" });
    scr.set_text("LBL_TITLE2", if creating { "Create New Items" } else { "Break Down Items" });
    let currency = if lab { Currency::Chemicals } else { Currency::Components };
    let have = w.currency_count(currency);
    scr.set_text("LBL_CREDITS", if lab { "Total Chemicals" } else { "Total Components" });
    scr.set_text("LBL_CREDITS_VALUE", have.to_string());
    let status = ui.status.clone();
    let (desc, cost, stock, skill_line) = if creating {
        scr.set_list(
            "LB_SHOPITEMS",
            recipes
                .iter()
                .map(|r| {
                    let icon = w.item_def(&r.template).and_then(|d| d.icon.clone());
                    ListItem { enabled: r.skill_ok && r.affordable, ..ListItem::text(r.name.clone()).with_icon(icon) }
                })
                .collect(),
        );
        if !recipes.is_empty() && scr.list_selected("LB_SHOPITEMS").is_none() {
            scr.set_list_selected("LB_SHOPITEMS", Some(0));
        }
        let shown = scr.hovered_item("LB_SHOPITEMS").or(scr.list_selected("LB_SHOPITEMS")).and_then(|i| recipes.get(i));
        match shown {
            Some(r) => {
                let d = w.item_def(&r.template);
                let text = d.as_ref().map(|d| if d.description.is_empty() { d.name.clone() } else { format!("{}\n\n{}", d.name, d.description) }).unwrap_or_default();
                let owned: i32 = w.party_inventory_views().iter().filter(|v| v.template.eq_ignore_ascii_case(&r.template)).map(|v| v.stack).sum();
                let rank = w.skill_rank(character, r.skill);
                let skill = format!("{}: {} (have {rank})", SKILL_NAMES.get(r.skill).copied().unwrap_or("Skill"), r.rank_required);
                (text, r.cost.to_string(), owned.to_string(), skill)
            }
            None => ("Nothing to create in this category.".into(), String::new(), String::new(), String::new()),
        }
    } else {
        scr.set_list("LB_INVITEMS", breakable.iter().map(|(v, _, n)| ListItem { text: format!("{}  (+{n})", v.name), ..item_row(v) }).collect());
        if !breakable.is_empty() && scr.list_selected("LB_INVITEMS").is_none() {
            scr.set_list_selected("LB_INVITEMS", Some(0));
        }
        let shown = scr.hovered_item("LB_INVITEMS").or(scr.list_selected("LB_INVITEMS")).and_then(|i| breakable.get(i));
        match shown {
            Some((v, _, n)) => (describe(v), format!("+{n}"), v.stack.to_string(), String::new()),
            None => ("Nothing here can be broken down.".into(), String::new(), String::new(), String::new()),
        }
    };
    scr.set_text("LBL_COST", if creating { if lab { "Chemical Cost" } else { "Component Cost" } } else { "Yield" });
    scr.set_text("LBL_COST_VALUE", cost);
    scr.set_text("LBL_STOCK_VALUE", stock);
    scr.set_text("LBL_RELATEDSKILL", skill_line);
    scr.set_text("LB_DESCRIPTION", if status.is_empty() { desc } else { format!("{status}\n\n{desc}") });
    let _: Option<ObjectId> = None;
    match next {
        Some("close") => close_menu(&mut commands, &mut open, &existing),
        Some(b) => open_menu(&mut commands, &game, &mut open, &existing, b),
        None => {}
    }
}

//! container_p (loot) and store_p.

use bevy::prelude::*;
use k2_world::inventory::ItemView;

use super::{Filter, ScreenState, classify, describe, item_row, passes};
use crate::game::WorldRes;
use crate::gui::menus::{MenuScreen, OpenMenu, close_menu};
use crate::gui::{GuiClick, GuiScreen, ListItem};
use crate::uisfx::UiSfx;

pub fn container(
    mut commands: Commands,
    mut world: ResMut<WorldRes>,
    mut open: ResMut<OpenMenu>,
    st: Res<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    existing: Query<Entity, With<MenuScreen>>,
    mut sfx: MessageWriter<UiSfx>,
) {
    if open.0.as_deref() != Some("container") {
        return;
    }
    let Some(c) = st.container else { return };
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("container")) else { return };
    let w = &mut world.0;
    let taker = w.party_leader();
    let mut close = false;
    for k in clicks.read().filter(|k| k.screen.starts_with("container")) {
        match (k.tag.as_str(), k.item) {
            ("LB_ITEMS", Some(i)) => {
                let items = w.container_views(c);
                if let Some(v) = items.get(i) {
                    let ok = w.take_item(c, v.id, taker).is_ok();
                    sfx.write(UiSfx(if ok { "Inventory_Add" } else { "Error_Default" }));
                }
            }
            ("BTN_OK", _) => {
                if !w.take_all(c, taker).is_empty() {
                    sfx.write(UiSfx("Inventory_Add"));
                }
                close = true;
            }
            ("BTN_CANCEL", _) => close = true,
            _ => {}
        }
    }
    let items = w.container_views(c);
    let name = w.get(c).map(|o| w.substitute(&o.name)).unwrap_or_default();
    scr.set_text("LBL_MESSAGE", if name.is_empty() { "Container".to_owned() } else { name });
    scr.set_list("LB_ITEMS", items.iter().map(item_row).collect());
    scr.set_visible("BTN_GIVEITEMS", false);
    scr.set_enabled("BTN_OK", !items.is_empty());
    if close {
        close_menu(&mut commands, &mut open, &existing);
    }
}

const STORE_FILTERS: &[(&str, Filter)] = &[("BTN_ALL", Filter::All), ("BTN_WEAPONS", Filter::Weapons), ("BTN_ARMOR", Filter::Armor), ("BTN_MISC", Filter::Misc)];

pub fn store(
    mut world: ResMut<WorldRes>,
    open: Res<OpenMenu>,
    mut st: ResMut<ScreenState>,
    mut clicks: MessageReader<GuiClick>,
    mut screens: Query<&mut GuiScreen, With<MenuScreen>>,
    mut sfx: MessageWriter<UiSfx>,
) {
    if open.0.as_deref() != Some("store") {
        return;
    }
    let Some(store) = st.store else { return };
    let Some(mut scr) = screens.iter_mut().find(|s| s.name.starts_with("store")) else { return };
    let w = &mut world.0;
    let Some(view) = w.store_view(store) else { return };
    let filter = st.store_filter;
    // Rows: (item, unit price, infinite).
    let rows_for = |w: &mut k2_world::World, sell: bool| -> Vec<(ItemView, i32, bool)> {
        let src: Vec<(ItemView, i32, bool)> =
            if sell { view.sellable.iter().map(|(v, p)| (v.clone(), *p, false)).collect() } else { view.stock.clone() };
        src.into_iter().filter(|(v, _, _)| passes(filter, classify(w, v))).collect()
    };
    let list_tag = |sell: bool| if sell { "LB_INVITEMS" } else { "LB_SHOPITEMS" };
    for k in clicks.read().filter(|k| k.screen.starts_with("store")) {
        if let Some(&(_, f)) = STORE_FILTERS.iter().find(|(t, _)| *t == k.tag) {
            st.store_filter = f;
            sfx.write(UiSfx("Checkbox_Check"));
            continue;
        }
        match k.tag.as_str() {
            "BTN_Examine" => {
                st.store_sell = !st.store_sell;
                st.status = None;
            }
            "BTN_Accept" => {
                let sell = st.store_sell;
                let rows = rows_for(w, sell);
                if let Some((v, _, _)) = scr.list_selected(list_tag(sell)).and_then(|i| rows.get(i)) {
                    st.status = Some(if sell {
                        match w.sell(store, v.id) {
                            Ok(p) => {
                                sfx.write(UiSfx("Inventory_Drop"));
                                format!("Sold {} for {p} credits.", v.name)
                            }
                            Err(e) => {
                                sfx.write(UiSfx("Error_Default"));
                                format!("{}: {e}", v.name)
                            }
                        }
                    } else {
                        match w.buy(store, v.id) {
                            Ok(_) => {
                                sfx.write(UiSfx("Inventory_Add"));
                                format!("Bought {}.", v.name)
                            }
                            Err(e) => {
                                sfx.write(UiSfx("Error_Default"));
                                format!("{}: {e}", v.name)
                            }
                        }
                    });
                }
            }
            _ => {}
        }
    }
    let sell = st.store_sell;
    for &(tag, f) in STORE_FILTERS {
        scr.set_selected(tag, st.store_filter == f);
    }
    scr.set_visible("LB_SHOPITEMS", !sell);
    scr.set_visible("LB_INVITEMS", sell);
    scr.set_text("LBL_BUYSELL", if sell { "Selling Items" } else { "Buying Items" });
    scr.set_text("BTN_Accept", if sell { "Sell Item" } else { "Buy Item" });
    scr.set_text("BTN_Examine", if sell { "Show Buy List" } else { "Show Sell List" });
    scr.set_enabled("BTN_Accept", if sell { view.can_sell } else { view.can_buy });
    let rows = rows_for(w, sell);
    let tag = list_tag(sell);
    scr.set_list(tag, rows.iter().map(|(v, p, _)| ListItem { text: format!("{}  ({p} cr)", v.name), ..item_row(v) }).collect());
    if !rows.is_empty() && scr.list_selected(tag).is_none() {
        scr.set_list_selected(tag, Some(0));
    }
    let shown = scr.hovered_item(tag).or(scr.list_selected(tag)).and_then(|i| rows.get(i));
    let on_hand = shown.map_or(0, |(v, _, _)| {
        let inv = w.party_inventory_views();
        inv.iter().filter(|i| i.template.eq_ignore_ascii_case(&v.template)).map(|i| i.stack).sum::<i32>()
    });
    scr.set_text("LBL_COST_VALUE", shown.map(|(_, p, _)| p.to_string()).unwrap_or_default());
    scr.set_text("LBL_CREDITS_VALUE", w.credits().to_string());
    scr.set_text("LBL_STOCK_VALUE", shown.map(|(v, _, inf)| if *inf { "-".to_owned() } else { v.stack.to_string() }).unwrap_or_default());
    scr.set_text("LBL_ONHAND_VALUE", on_hand.to_string());
    let mut desc = shown.map(|(v, _, _)| describe(v)).unwrap_or_else(|| view.name.clone());
    if let Some(s) = &st.status {
        desc = format!("{s}\n\n{desc}");
    }
    scr.set_text("LB_DESCRIPTION", desc);
}

//! Merchants (UTM stores): stock, prices, buying and selling.
//!
//! A GIT StoreList entry spawns an [`ObjectKind::Store`] object; its UTM
//! (the TemplateResRef) is read the first time the store opens. Stock items
//! become item objects owned by the store. `Infinite` stock never runs out:
//! buying one creates a fresh copy.
//!
//! Prices: buying costs value * (MarkUp + bonus) / 100, selling pays
//! value * (MarkDown + bonus) / 100, per unit, at least 1. BuySellFlag bit 0
//! allows the player to buy, bit 1 to sell. Plot items cannot be sold.

use std::collections::HashSet;

use k2_data::GffExt;
use k2_script::{ObjectId, OBJECT_INVALID};
use rakata_core::ResourceType;

use crate::inventory::{ItemError, ItemView};
use crate::object::ObjectKind;
use crate::{ScriptCtx, World, WorldEvent};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct StoreInfo {
    pub name: String,
    pub mark_up: i32,
    pub mark_down: i32,
    /// BuySellFlag: bit 0 player may buy, bit 1 player may sell.
    pub buy_sell: u32,
    pub on_open: Option<String>,
    /// Stock items that never run out.
    pub infinite: HashSet<ObjectId>,
}

/// What the store window shows.
#[derive(Debug, Clone)]
pub struct StoreView {
    pub store: ObjectId,
    pub name: String,
    /// Stock with the price the player pays for one unit.
    pub stock: Vec<(ItemView, i32, bool)>,
    /// The party's sellable items with the price the store pays per unit.
    pub sellable: Vec<(ItemView, i32)>,
    pub credits: i32,
    pub can_buy: bool,
    pub can_sell: bool,
}

impl World {
    /// Reads the store's UTM and stocks it (once per store object).
    fn ensure_store(&mut self, store: ObjectId) -> bool {
        if self.inventory.stores.contains_key(&store) {
            return true;
        }
        let Some(template) = self.get(store).filter(|o| o.kind == ObjectKind::Store).map(|o| o.template.clone()) else {
            return false;
        };
        let Ok(gff) = k2_data::read_gff(&self.res, &template, ResourceType::Utm) else {
            self.warnings.push(format!("store {template}: no UTM"));
            return false;
        };
        let r = &gff.root;
        let mut info = StoreInfo {
            name: self.talk.as_ref().map(|t| t.resolve(&r.locstring("LocName"))).unwrap_or_default(),
            mark_up: r.int("MarkUp").unwrap_or(100),
            mark_down: r.int("MarkDown").unwrap_or(50),
            buy_sell: r.uint("BuySellFlag").unwrap_or(3),
            on_open: r.resref("OnOpenStore").filter(|s| !s.is_empty()),
            infinite: HashSet::new(),
        };
        let stock: Vec<(String, bool)> =
            r.list("ItemList").iter().filter_map(|e| Some((e.resref("InventoryRes")?, e.flag("Infinite")))).collect();
        for (res, infinite) in stock {
            if let Some(id) = self.create_item(&res, store, 1) {
                if infinite {
                    info.infinite.insert(id);
                }
            } else {
                self.warnings.push(format!("store {template}: missing item {res}"));
            }
        }
        if let Some(o) = self.get_mut(store)
            && o.name.is_empty()
        {
            o.name = info.name.clone();
        }
        self.inventory.stores.insert(store, info);
        true
    }

    /// OpenStore: stocks the store, runs its OnOpenStore script and asks
    /// the presentation layer for the store window.
    pub fn open_store(&mut self, store: ObjectId, customer: ObjectId, bonus_mark_up: i32, bonus_mark_down: i32) {
        if !self.ensure_store(store) {
            return;
        }
        if let Some(script) = self.inventory.stores.get(&store).and_then(|s| s.on_open.clone()) {
            self.run_script(&script, store, ScriptCtx { last_used_by: customer, ..Default::default() });
        }
        self.inventory.open_store = Some((store, bonus_mark_up, bonus_mark_down));
        self.emit(WorldEvent::OpenStore { store });
    }

    pub fn close_store(&mut self) {
        self.inventory.open_store = None;
    }

    fn bonuses(&self, store: ObjectId) -> (i32, i32) {
        match self.inventory.open_store {
            Some((s, up, down)) if s == store => (up, down),
            _ => (0, 0),
        }
    }

    /// Credits the player pays for one unit of `item` here.
    pub fn buy_price(&mut self, store: ObjectId, item: ObjectId) -> i32 {
        let value = self.item_view(item).map_or(0, |v| v.value);
        let up = self.inventory.stores.get(&store).map_or(100, |s| s.mark_up) + self.bonuses(store).0;
        (value * up / 100).max(1)
    }

    /// Credits the store pays for one unit of `item`.
    pub fn sell_price(&mut self, store: ObjectId, item: ObjectId) -> i32 {
        let value = self.item_view(item).map_or(0, |v| v.value);
        let down = self.inventory.stores.get(&store).map_or(50, |s| s.mark_down) + self.bonuses(store).1;
        (value * down / 100).max(1)
    }

    pub fn store_view(&mut self, store: ObjectId) -> Option<StoreView> {
        if !self.ensure_store(store) {
            return None;
        }
        let info = self.inventory.stores.get(&store)?.clone();
        let stock_ids = self.inventory_of(store);
        let mut stock = Vec::new();
        for id in stock_ids {
            let price = self.buy_price(store, id);
            if let Some(v) = self.item_view(id) {
                stock.push((v, price, info.infinite.contains(&id)));
            }
        }
        let mut sellable = Vec::new();
        for id in self.party_inventory() {
            let price = self.sell_price(store, id);
            if let Some(v) = self.item_view(id).filter(|v| !v.plot) {
                sellable.push((v, price));
            }
        }
        Some(StoreView {
            store,
            name: info.name.clone(),
            stock,
            sellable,
            credits: self.credits(),
            can_buy: info.buy_sell & 1 != 0,
            can_sell: info.buy_sell & 2 != 0,
        })
    }

    /// Buys one unit of a stock item for the party.
    pub fn buy(&mut self, store: ObjectId, item: ObjectId) -> Result<ObjectId, ItemError> {
        if !self.ensure_store(store) || !self.inventory_of(store).contains(&item) {
            return Err(ItemError::NoItem);
        }
        let info = self.inventory.stores.get(&store).cloned().unwrap_or_default();
        if info.buy_sell & 1 == 0 {
            return Err(ItemError::StoreRefuses("sell to you"));
        }
        let price = self.buy_price(store, item);
        if self.credits() < price {
            return Err(ItemError::NoCredits);
        }
        self.add_credits(-price);
        let pc = self.player;
        let units = self.get(item).and_then(|o| o.item()).map_or(1, |i| i.stack_size);
        let bought = if info.infinite.contains(&item) || units > 1 {
            if units > 1
                && !info.infinite.contains(&item)
                && let Some(s) = self.get_mut(item).and_then(|o| o.item_mut())
            {
                s.stack_size -= 1;
            }
            let template = self.get(item).map(|o| o.template.clone()).unwrap_or_default();
            self.give_new_item(&template, pc, 1, true).ok_or(ItemError::NoItem)?
        } else {
            self.store_item(item, pc)
        };
        Ok(bought)
    }

    /// Sells one unit of a party item to the store.
    pub fn sell(&mut self, store: ObjectId, item: ObjectId) -> Result<i32, ItemError> {
        if !self.ensure_store(store) || !self.party_inventory().contains(&item) {
            return Err(ItemError::NoItem);
        }
        let info = self.inventory.stores.get(&store).cloned().unwrap_or_default();
        if info.buy_sell & 2 == 0 {
            return Err(ItemError::StoreRefuses("buy from you"));
        }
        if self.item_view(item).is_some_and(|v| v.plot) {
            return Err(ItemError::Plot);
        }
        let price = self.sell_price(store, item);
        let units = self.get(item).and_then(|o| o.item()).map_or(1, |i| i.stack_size);
        if units > 1 {
            if let Some(s) = self.get_mut(item).and_then(|o| o.item_mut()) {
                s.stack_size -= 1;
            }
            let template = self.get(item).map(|o| o.template.clone()).unwrap_or_default();
            if let Some(one) = self.create_item(&template, OBJECT_INVALID, 1) {
                self.store_item(one, store);
            }
        } else {
            self.store_item(item, store);
        }
        self.add_credits(price);
        Ok(price)
    }
}

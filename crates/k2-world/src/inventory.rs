//! Items, the shared party inventory, equipment and containers.
//!
//! K2 shares one inventory across the party. It lives on the player
//! character object ([`World::player`]), also while SwitchPlayerCharacter
//! has the PC stashed: anything given to any party member lands there.
//! Equipment stays per creature ([`crate::object::CreatureState::equipped`],
//! INVENTORY_SLOT_* -> item). Combat numbers are recomputed from equipment
//! on every query, so equipping needs no extra bookkeeping.
//!
//! Item templates are described by [`ItemDef`] (UTI plus baseitems.2da),
//! cached per template. Presentation reads [`ItemView`]s.

use std::collections::HashMap;
use std::sync::Arc;

use k2_data::{GffExt, LocString};
use k2_script::{ObjectId, Value, OBJECT_INVALID};
use rakata_core::ResourceType;

use crate::object::ObjectKind;
use crate::stats::{SLOT_BODY, SLOT_CWEAPON_B, SLOT_CWEAPON_L, SLOT_CWEAPON_R, SLOT_LEFTWEAPON, SLOT_LEFTWEAPON2, SLOT_RIGHTWEAPON, SLOT_RIGHTWEAPON2};
use crate::{ScriptCtx, World, WorldEvent};
use serde::{Deserialize, Serialize};

/// INVENTORY_SLOT_* count (0 head .. 17 creature armour).
pub const NUM_SLOTS: i32 = 18;

/// Seconds an equip, unequip or weapon-set swap keeps the hands busy: the
/// length of the creature "equip" clip (S_Male02, draw_weapon event at 0.57 s).
pub const EQUIP_SECONDS: f32 = 1.0;

/// The slot whose equipableslots bit a weapon-set-2 slot answers to.
fn weapon_base_slot(slot: i32) -> i32 {
    match slot {
        SLOT_RIGHTWEAPON2 => SLOT_RIGHTWEAPON,
        SLOT_LEFTWEAPON2 => SLOT_LEFTWEAPON,
        s => s,
    }
}

/// Slot names for logs and UIs, by INVENTORY_SLOT_* index.
pub const SLOT_NAMES: [&str; 18] = [
    "head", "body", "", "hands", "right weapon", "left weapon", "", "left arm", "right arm", "implant", "belt", "", "", "",
    "creature weapon L", "creature weapon R", "creature weapon B", "creature armour",
];


/// One item template: UTI fields plus what baseitems.2da says about its base.
#[derive(Debug, Clone, Default)]
pub struct ItemDef {
    pub template: String,
    pub tag: String,
    pub name: String,
    /// DescIdentified, falling back to Description.
    pub description: String,
    pub base_item: u32,
    /// baseitems label, e.g. "Blaster_Pistol".
    pub base_label: String,
    /// baseitems itemclass, lowercase, e.g. "w_blstrpstl".
    pub item_class: String,
    pub model_variation: u32,
    pub texture_variation: u32,
    pub body_variation: u32,
    pub stack_size: i32,
    /// baseitems stacking: the largest stack (1 = does not stack).
    pub max_stack: i32,
    pub charges: i32,
    /// UTI Cost (the toolset's precomputed value; AddCost is a copy in the
    /// shipped data, so it is not added again).
    pub cost: i32,
    pub plot: bool,
    pub stolen: bool,
    pub identified: bool,
    /// K2 upgrade tier (number of upgrade slots the item takes).
    pub upgrade_level: i32,
    pub property_count: usize,
    /// baseitems equipableslots bitmask (bit n = INVENTORY_SLOT n).
    pub equip_slots: u32,
    /// baseitems droidorhuman: 0 either, 1 organics only, 2 droids only.
    pub droid_or_human: i32,
    /// baseitems weaponwield (see `WeaponInfo::wield`); 0 for non-weapons.
    pub weapon_wield: i32,
    /// Inventory icon texture (resolved to one that exists), if any.
    pub icon: Option<String>,
    /// Model to show in hand (weapons) or on the ground, if any.
    pub model: Option<String>,
}

impl ItemDef {
    pub fn is_weapon(&self) -> bool {
        self.equip_slots & ((1 << SLOT_RIGHTWEAPON) | (1 << SLOT_LEFTWEAPON)) != 0 && self.equip_slots & (1 << 14) == 0
    }

    /// Two-handed: fits the right hand but not the left.
    pub fn two_handed(&self) -> bool {
        self.equip_slots & (1 << SLOT_RIGHTWEAPON) != 0 && self.equip_slots & (1 << SLOT_LEFTWEAPON) == 0
    }

    pub fn fits(&self, slot: i32) -> bool {
        (0..32).contains(&slot) && self.equip_slots & (1 << slot) != 0
    }

    /// Which weapons may be paired in the two hands: 1 one-handed melee
    /// (stun baton and blades, lightsabers included), 2 pistols, 0 never
    /// (two-handed, rifles, creature weapons, non-weapons).
    pub fn dual_class(&self) -> u8 {
        if !self.is_weapon() || self.two_handed() {
            return 0;
        }
        match self.weapon_wield {
            1 | 2 => 1,
            4 => 2,
            _ => 0,
        }
    }
}

/// An item as the UI shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct ItemView {
    pub id: ObjectId,
    pub template: String,
    pub tag: String,
    pub name: String,
    pub description: String,
    pub icon: Option<String>,
    pub stack: i32,
    pub base_item: u32,
    pub base_label: String,
    pub equip_slots: u32,
    /// Value of one unit in credits.
    pub value: i32,
    pub plot: bool,
    pub upgrade_level: i32,
    pub charges: i32,
}

/// Why an equip or container action failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ItemError {
    #[error("no such item")]
    NoItem,
    #[error("not a creature")]
    NoCreature,
    #[error("item does not fit slot {0}")]
    WrongSlot(i32),
    #[error("droids cannot use this")]
    DroidOnly,
    #[error("only droids can use this")]
    OrganicOnly,
    #[error("the right hand holds a two-handed weapon")]
    TwoHanded,
    #[error("the off hand needs a one-handed weapon in the main hand")]
    NeedsMainHand,
    #[error("these weapons cannot be wielded together")]
    NoDualWield,
    #[error("locked")]
    Locked,
    #[error("not a container")]
    NotContainer,
    #[error("not enough credits")]
    NoCredits,
    #[error("the store does not {0}")]
    StoreRefuses(&'static str),
    #[error("plot items cannot be sold")]
    Plot,
}

/// Item catalog and container/store bookkeeping.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Inventories {
    #[serde(skip)]
    defs: HashMap<String, Option<Arc<ItemDef>>>,
    /// The container whose loot window is open, if any.
    pub open_container: Option<ObjectId>,
    pub(crate) stores: HashMap<ObjectId, crate::store::StoreInfo>,
    /// The store whose window is open, with its bonus markup/markdown.
    pub open_store: Option<(ObjectId, i32, i32)>,
    /// Item last moved by a disturbance (GetInventoryDisturbItem).
    pub last_disturbed: Option<ObjectId>,
}

/// Drops `{...}` developer notes (K2 item names carry tier tags like
/// "{01}Blaster Pistol") and trims.
pub fn strip_notes(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut depth = 0;
    for ch in s.chars() {
        match ch {
            '{' => depth += 1,
            '}' if depth > 0 => depth -= 1,
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    out.trim().to_owned()
}

fn int(a: &[Value], n: usize) -> i32 {
    match a.get(n) {
        Some(Value::Int(v)) => *v,
        Some(Value::Float(v)) => *v as i32,
        _ => 0,
    }
}

fn string(a: &[Value], n: usize) -> &str {
    match a.get(n) {
        Some(Value::String(s)) => s,
        _ => "",
    }
}

fn obj(a: &[Value], n: usize) -> ObjectId {
    match a.get(n) {
        Some(Value::Object(o)) => *o,
        _ => OBJECT_INVALID,
    }
}

fn o_opt(v: Option<ObjectId>) -> Value {
    Value::Object(v.unwrap_or(OBJECT_INVALID))
}

impl World {
    // ---------------------------------------------------------------
    // Item templates
    // ---------------------------------------------------------------

    /// The template description, cached. None for a missing UTI.
    pub fn item_def(&mut self, template: &str) -> Option<Arc<ItemDef>> {
        let key = template.to_ascii_lowercase();
        if let Some(d) = self.inventory.defs.get(&key) {
            return d.clone();
        }
        let def = self.load_item_def(&key).map(Arc::new);
        self.inventory.defs.insert(key, def.clone());
        def
    }

    fn load_item_def(&self, template: &str) -> Option<ItemDef> {
        let gff = match k2_data::read_gff(&self.res, template, ResourceType::Uti) {
            Ok(gff) => gff,
            // An item keeps the template of the area it was found in, and a save
            // restores items into its own area, whose module may not hold that
            // UTI: every module is searched.
            Err(_) => {
                let key = k2_res::ResKey::new(template, ResourceType::Uti);
                let bytes = self.res.install.module_names().iter().find_map(|m| self.res.install.read_module_resource(m, &key).ok().flatten())?;
                rakata_formats::read_gff_from_bytes(&bytes).ok()?
            }
        };
        let r = &gff.root;
        let base_item = r.int("BaseItem").unwrap_or(0).max(0) as u32;
        let name = self.resolve_loc(&r.locstring("LocalizedName"));
        let mut description = self.resolve_loc(&r.locstring("DescIdentified"));
        if description.is_empty() {
            description = self.resolve_loc(&r.locstring("Description"));
        }
        let mut d = ItemDef {
            template: template.to_owned(),
            tag: r.string_or_empty("Tag"),
            name,
            description,
            base_item,
            model_variation: r.uint("ModelVariation").unwrap_or(1),
            texture_variation: r.uint("TextureVar").unwrap_or(1),
            body_variation: r.uint("BodyVariation").unwrap_or(0),
            stack_size: r.int("StackSize").unwrap_or(1).max(1),
            charges: r.int("Charges").unwrap_or(0),
            cost: r.int("Cost").unwrap_or(0),
            plot: r.flag("Plot"),
            stolen: r.flag("Stolen"),
            identified: r.flag("Identified"),
            upgrade_level: r.int("UpgradeLevel").unwrap_or(0),
            property_count: r.list("PropertiesList").len(),
            max_stack: 1,
            ..Default::default()
        };
        if let Ok(t) = self.tables.get(&self.res, "baseitems") {
            let row = base_item as usize;
            d.base_label = t.str(row, "label").unwrap_or("").to_owned();
            d.item_class = t.resref(row, "itemclass").unwrap_or_default();
            d.max_stack = t.int(row, "stacking").unwrap_or(1).max(1);
            d.equip_slots = t.int(row, "equipableslots").unwrap_or(0).max(0) as u32;
            d.droid_or_human = t.int(row, "droidorhuman").unwrap_or(0);
            d.weapon_wield = t.int(row, "weaponwield").unwrap_or(0);
            d.icon = self.item_icon(&d.item_class, d.model_variation, t.resref(row, "defaulticon"));
            d.model = self.item_model(&d.item_class, d.model_variation, t.resref(row, "defaultmodel"));
        }
        Some(d)
    }

    fn resolve_loc(&self, s: &LocString) -> String {
        strip_notes(&self.talk.as_ref().map(|t| t.resolve(s)).unwrap_or_default())
    }

    /// Inventory icon: "i" + itemclass + "_" + %03d model variation (the
    /// convention every shipped item follows), else baseitems defaulticon.
    fn item_icon(&self, class: &str, variation: u32, default: Option<String>) -> Option<String> {
        let cands = [
            (!class.is_empty()).then(|| format!("i{class}_{variation:03}")),
            (!class.is_empty()).then(|| format!("i{class}_001")),
            default,
        ];
        cands.into_iter().flatten().find(|c| k2_data::tables::texture_exists(&self.res, c))
    }

    /// Weapon/ground model: itemclass + "_" + %03d variation, else the
    /// baseitems defaultmodel (I_Null means none).
    fn item_model(&self, class: &str, variation: u32, default: Option<String>) -> Option<String> {
        let exists = |m: &str| self.res.locate(&k2_res::ResKey::new(m, ResourceType::Mdl)).is_some();
        let cands = [
            (!class.is_empty()).then(|| format!("{class}_{variation:03}")),
            default.filter(|d| d != "i_null"),
        ];
        cands.into_iter().flatten().find(|m| exists(m))
    }

    /// Template description of a live item object.
    pub fn item_def_of(&mut self, item: ObjectId) -> Option<Arc<ItemDef>> {
        let template = self.get(item).filter(|o| o.kind == ObjectKind::Item)?.template.clone();
        self.item_def(&template)
    }

    pub fn item_view(&mut self, item: ObjectId) -> Option<ItemView> {
        let o = self.get(item).filter(|o| o.kind == ObjectKind::Item)?;
        let (name, tag) = (o.name.clone(), o.tag.clone());
        let st = o.item().cloned().unwrap_or_default();
        let def = self.item_def_of(item).unwrap_or_default();
        Some(ItemView {
            id: item,
            template: def.template.clone(),
            tag,
            name: if name.is_empty() { def.name.clone() } else { strip_notes(&name) },
            description: def.description.clone(),
            icon: def.icon.clone(),
            stack: st.stack_size,
            base_item: st.base_item,
            base_label: def.base_label.clone(),
            equip_slots: def.equip_slots,
            value: def.cost.max(0),
            plot: st.plot || def.plot,
            upgrade_level: def.upgrade_level,
            charges: def.charges,
        })
    }

    // ---------------------------------------------------------------
    // Party inventory
    // ---------------------------------------------------------------

    /// Whether `id` shares the party inventory.
    pub fn is_party_member(&self, id: ObjectId) -> bool {
        id != OBJECT_INVALID && (id == self.player || self.party.contains(&id))
    }

    fn holder_inventory(&self) -> Option<&Vec<ObjectId>> {
        match self.objects.get(&self.player) {
            Some(o) if !o.destroyed => Some(&o.inventory),
            _ => self.stashed_pc.as_ref().filter(|p| p.id == self.player).map(|p| &p.inventory),
        }
    }

    fn holder_inventory_mut(&mut self) -> Option<&mut Vec<ObjectId>> {
        let pc = self.player;
        match self.objects.get_mut(&pc) {
            Some(o) if !o.destroyed => Some(&mut o.inventory),
            _ => self.stashed_pc.as_mut().filter(|p| p.id == pc).map(|p| &mut p.inventory),
        }
    }

    /// The shared party inventory (not counting equipped items).
    pub fn party_inventory(&self) -> Vec<ObjectId> {
        self.holder_inventory().map(|v| v.iter().copied().filter(|&i| self.is_valid(i)).collect()).unwrap_or_default()
    }

    pub fn party_inventory_views(&mut self) -> Vec<ItemView> {
        self.party_inventory().into_iter().filter_map(|i| self.item_view(i)).collect()
    }

    /// Party credits (kept on the PC).
    pub fn credits(&self) -> i32 {
        let pc = self.player;
        let o = match self.objects.get(&pc) {
            Some(o) if !o.destroyed => Some(o),
            _ => self.stashed_pc.as_ref(),
        };
        o.and_then(|o| o.creature()).map_or(0, |c| c.gold)
    }

    pub fn add_credits(&mut self, delta: i32) {
        let pc = self.player;
        let o = match self.objects.get_mut(&pc) {
            Some(o) if !o.destroyed => Some(o),
            _ => self.stashed_pc.as_mut(),
        };
        if let Some(c) = o.and_then(|o| o.creature_mut()) {
            c.gold = (c.gold + delta).max(0);
        }
    }

    /// Takes `item` away from whoever holds it (inventory or equipment).
    pub(crate) fn detach_item(&mut self, item: ObjectId) {
        let Some(old) = self.get(item).and_then(|o| o.item()).and_then(|i| i.possessor) else { return };
        let was_equipped = self.get(old).and_then(|o| o.creature()).is_some_and(|c| c.equipped.values().any(|&x| x == item));
        let strip = |o: &mut crate::Object| {
            o.inventory.retain(|&x| x != item);
            if let Some(c) = o.creature_mut() {
                c.equipped.retain(|_, &mut x| x != item);
            }
        };
        if let Some(p) = self.objects.get_mut(&old) {
            strip(p);
        } else if let Some(p) = self.stashed_pc.as_mut().filter(|p| p.id == old) {
            strip(p);
        }
        if let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) {
            it.possessor = None;
        }
        if was_equipped {
            self.repair_weapon_slots(old);
        }
    }

    /// Puts `item` into `owner`'s inventory, redirecting party members to
    /// the shared party inventory and merging stacks of the same template.
    /// Returns the object that now holds the units (the merged stack or
    /// `item` itself).
    pub(crate) fn store_item(&mut self, item: ObjectId, owner: ObjectId) -> ObjectId {
        self.detach_item(item);
        let party = self.is_party_member(owner);
        let owner = if party { self.player } else { owner };
        // Merge into an existing stack when the base item stacks.
        let template = self.get(item).map(|o| o.template.clone()).unwrap_or_default();
        let max = self.item_def(&template).map_or(1, |d| d.max_stack);
        // Upgraded items (craft.rs) are unique and never merge.
        if max > 1 && !self.craft.installed.contains_key(&item) {
            let units = self.get(item).and_then(|o| o.item()).map_or(1, |i| i.stack_size);
            let list = if party { self.party_inventory() } else { self.get(owner).map(|o| o.inventory.clone()).unwrap_or_default() };
            let target = list.into_iter().find(|&i| {
                i != item
                    && !self.craft.installed.contains_key(&i)
                    && self.get(i).is_some_and(|o| o.template == template)
                    && self.get(i).and_then(|o| o.item()).is_some_and(|s| s.stack_size + units <= max)
            });
            if let Some(t) = target {
                if let Some(s) = self.get_mut(t).and_then(|o| o.item_mut()) {
                    s.stack_size += units;
                }
                if let Some(o) = self.objects.get_mut(&item) {
                    o.destroyed = true;
                }
                return t;
            }
        }
        if let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) {
            it.possessor = Some(owner);
        }
        if party {
            if let Some(inv) = self.holder_inventory_mut()
                && !inv.contains(&item)
            {
                inv.push(item);
            }
        } else if let Some(o) = self.get_mut(owner)
            && !o.inventory.contains(&item)
        {
            o.inventory.push(item);
        }
        item
    }

    /// Creates an item for the party (or anyone), announcing it like the
    /// engine's "item received" feedback unless `quiet`.
    pub fn give_new_item(&mut self, template: &str, owner: ObjectId, stack: i32, quiet: bool) -> Option<ObjectId> {
        let id = self.create_item(template, OBJECT_INVALID, stack.max(1))?;
        if let Some(it) = self.get_mut(id).and_then(|o| o.item_mut()) {
            it.stack_size = stack.max(1);
        }
        let party = self.is_party_member(owner);
        let held = self.store_item(id, owner);
        if party && !quiet {
            let name = self.get(held).map(|o| strip_notes(&o.name)).unwrap_or_default();
            self.emit(WorldEvent::ItemReceived { item: held, name, stack: stack.max(1) });
        }
        Some(held)
    }

    /// Items a creature or container holds, party-aware: party members see
    /// the shared inventory.
    pub fn inventory_of(&self, id: ObjectId) -> Vec<ObjectId> {
        if self.is_party_member(id) {
            return self.party_inventory();
        }
        self.get(id).map(|o| o.inventory.iter().copied().filter(|&i| self.is_valid(i)).collect()).unwrap_or_default()
    }

    /// Equipped items of a creature, by slot (sorted).
    pub fn equipment(&self, id: ObjectId) -> Vec<(i32, ObjectId)> {
        let mut v: Vec<(i32, ObjectId)> = self
            .get(id)
            .and_then(|o| o.creature())
            .map(|c| c.equipped.iter().map(|(&s, &i)| (s, i)).filter(|&(_, i)| self.is_valid(i)).collect())
            .unwrap_or_default();
        v.sort();
        v
    }

    /// GetItemPossessedBy semantics: the holder's inventory (shared for the
    /// party) plus its own equipment.
    pub fn find_possessed(&self, owner: ObjectId, tag: &str) -> Option<ObjectId> {
        self.inventory_of(owner)
            .into_iter()
            .chain(self.equipment(owner).into_iter().map(|(_, i)| i))
            .find(|&i| self.get(i).is_some_and(|o| o.tag.eq_ignore_ascii_case(tag)))
    }

    // ---------------------------------------------------------------
    // Equipment
    // ---------------------------------------------------------------

    fn is_droid(&self, id: ObjectId) -> bool {
        self.get(id).and_then(|o| o.creature()).is_some_and(|c| c.racial_type == 5)
    }

    /// Checks whether `creature` may wear `item` in `slot`.
    pub fn can_equip(&mut self, creature: ObjectId, item: ObjectId, slot: i32) -> Result<(), ItemError> {
        if self.get(creature).and_then(|o| o.creature()).is_none() {
            return Err(ItemError::NoCreature);
        }
        let def = self.item_def_of(item).ok_or(ItemError::NoItem)?;
        if !def.fits(weapon_base_slot(slot)) {
            return Err(ItemError::WrongSlot(slot));
        }
        match (def.droid_or_human, self.is_droid(creature)) {
            (1, true) => return Err(ItemError::OrganicOnly),
            (2, false) => return Err(ItemError::DroidOnly),
            _ => {}
        }
        // The off hand needs a one-handed weapon in the right hand of its own
        // set, and a partner of the same kind (melee with melee, pistol with
        // pistol).
        if weapon_base_slot(slot) == SLOT_LEFTWEAPON {
            let right = self.equipment(creature).into_iter().find(|&(s, _)| s == slot - 1).map(|(_, i)| i);
            let Some(right_def) = right.filter(|&r| r != item).and_then(|r| self.item_def_of(r)) else {
                return Err(ItemError::NeedsMainHand);
            };
            if right_def.two_handed() {
                return Err(ItemError::TwoHanded);
            }
            if right_def.dual_class() == 0 || right_def.dual_class() != def.dual_class() {
                return Err(ItemError::NoDualWield);
            }
        }
        Ok(())
    }

    /// Party inventory items `creature` could put in `slot` right now, in
    /// inventory order (slot mask, droid/organic and the two-handed rule
    /// via `can_equip`). Backs the equipment screen's slot list.
    pub fn equip_candidates(&mut self, creature: ObjectId, slot: i32) -> Vec<ItemView> {
        let all = self.party_inventory_views();
        all.into_iter().filter(|v| self.can_equip(creature, v.id, slot).is_ok()).collect()
    }

    /// Equips `item` (from anywhere) on `creature` in `slot`. Whatever was
    /// in the slot goes back to the inventory. A new main-hand weapon that
    /// cannot be paired with the off-hand weapon (two-handed, or another
    /// kind) sends the off-hand weapon back to the inventory too; an
    /// off-hand weapon needs a main-hand partner (`can_equip`).
    pub fn equip(&mut self, creature: ObjectId, item: ObjectId, slot: i32) -> Result<(), ItemError> {
        self.can_equip(creature, item, slot)?;
        // A stack: equip one unit.
        let units = self.get(item).and_then(|o| o.item()).map_or(1, |i| i.stack_size);
        let item = if units > 1 {
            let template = self.get(item).map(|o| o.template.clone()).unwrap_or_default();
            if let Some(s) = self.get_mut(item).and_then(|o| o.item_mut()) {
                s.stack_size -= 1;
            }
            self.create_item(&template, OBJECT_INVALID, 1).ok_or(ItemError::NoItem)?
        } else {
            item
        };
        self.unequip_raw(creature, slot);
        self.detach_item(item);
        if let Some(c) = self.get_mut(creature).and_then(|o| o.creature_mut()) {
            c.equipped.insert(slot, item);
        }
        if let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) {
            it.possessor = Some(creature);
        }
        self.equipment_changed(creature, slot);
        if weapon_base_slot(slot) == SLOT_RIGHTWEAPON {
            let main = self.item_def_of(item);
            let off = self.equipment(creature).into_iter().find(|&(s, _)| s == slot + 1).map(|(_, i)| i);
            if let Some(off) = off
                && !main.is_some_and(|m| m.dual_class() != 0 && self.item_def_of(off).is_some_and(|o| o.dual_class() == m.dual_class()))
            {
                self.unequip_raw(creature, slot + 1);
            }
        }
        self.begin_draw(creature, slot);
        Ok(())
    }

    /// Moves the item in `slot` back to the (party) inventory. Taking the
    /// main-hand weapon away while an off-hand weapon is held moves the
    /// off-hand weapon to the main hand (the off hand never holds a weapon
    /// alone).
    pub fn unequip(&mut self, creature: ObjectId, slot: i32) -> Option<ObjectId> {
        let item = self.unequip_raw(creature, slot)?;
        self.repair_weapon_slots(creature);
        self.begin_draw(creature, slot);
        Some(item)
    }

    /// A live hand slot (4/5) changed: the creature spends `EQUIP_SECONDS`
    /// drawing, and the presentation plays its equip clip.
    fn begin_draw(&mut self, creature: ObjectId, slot: i32) {
        if slot != SLOT_RIGHTWEAPON && slot != SLOT_LEFTWEAPON {
            return;
        }
        let until = self.time + f64::from(EQUIP_SECONDS);
        if let Some(c) = self.get_mut(creature).and_then(|o| o.creature_mut()).filter(|c| !c.dead) {
            c.equip_until = until;
        }
    }

    /// Seconds left before `creature` finishes drawing a weapon (0 when
    /// idle). Attacks wait; the presentation shows the equip clip meanwhile.
    pub fn weapon_busy(&self, creature: ObjectId) -> f32 {
        self.get(creature).and_then(|o| o.creature()).map_or(0.0, |c| (c.equip_until - self.time).max(0.0) as f32)
    }

    /// Enforces the hand rules on both weapon sets: an off-hand weapon
    /// without a main-hand weapon moves to the main hand, and one beside a
    /// two-handed main-hand weapon goes back to the inventory. Used after
    /// anything that can empty a hand and when loading old saves.
    pub fn repair_weapon_slots(&mut self, creature: ObjectId) {
        for (right, left) in [(SLOT_RIGHTWEAPON, SLOT_LEFTWEAPON), (SLOT_RIGHTWEAPON2, SLOT_LEFTWEAPON2)] {
            let held = |w: &Self, slot: i32| w.get(creature).and_then(|o| o.creature()).and_then(|c| c.equipped.get(&slot).copied());
            let Some(off) = held(self, left) else { continue };
            match held(self, right) {
                None => {
                    if let Some(c) = self.get_mut(creature).and_then(|o| o.creature_mut()) {
                        c.equipped.remove(&left);
                        c.equipped.insert(right, off);
                    }
                    self.equipment_changed(creature, right);
                    self.equipment_changed(creature, left);
                }
                Some(main) if self.item_def_of(main).is_some_and(|d| d.two_handed()) => {
                    self.unequip_raw(creature, left);
                }
                Some(_) => {}
            }
        }
    }

    /// Repairs every creature's weapon slots (after loading a save).
    pub(crate) fn repair_all_weapon_slots(&mut self) {
        let ids: Vec<ObjectId> = self.iter().filter(|o| o.creature().is_some()).map(|o| o.id).collect();
        for id in ids {
            self.repair_weapon_slots(id);
        }
    }

    fn unequip_raw(&mut self, creature: ObjectId, slot: i32) -> Option<ObjectId> {
        let item = self.get_mut(creature).and_then(|o| o.creature_mut()).and_then(|c| c.equipped.remove(&slot))?;
        self.store_item(item, creature);
        self.equipment_changed(creature, slot);
        Some(item)
    }

    /// Active weapon set of a creature: 0 or 1.
    pub fn weapon_set(&self, creature: ObjectId) -> u8 {
        self.get(creature).and_then(|o| o.creature()).map_or(0, |c| c.weapon_set)
    }

    /// (right, left) inventory slots that hold weapon set `set` (0 or 1)
    /// right now: the live slots 4/5 for the active set, 18/19 otherwise.
    pub fn weapon_set_slots(&self, creature: ObjectId, set: u8) -> (i32, i32) {
        if set == self.weapon_set(creature) { (SLOT_RIGHTWEAPON, SLOT_LEFTWEAPON) } else { (SLOT_RIGHTWEAPON2, SLOT_LEFTWEAPON2) }
    }

    /// True when the inactive weapon set holds a weapon, i.e. there is a set
    /// to swap to (ActionSwitchWeapons does nothing otherwise).
    pub fn other_weapon_set_armed(&self, creature: ObjectId) -> bool {
        self.get(creature).and_then(|o| o.creature()).is_some_and(|c| c.equipped.contains_key(&SLOT_RIGHTWEAPON2) || c.equipped.contains_key(&SLOT_LEFTWEAPON2))
    }

    /// Weapon wield class (baseitems weaponwield) of the item in `slot`, when it
    /// is a weapon. Reads only cached data, so animation can ask it.
    pub fn held_wield(&self, creature: ObjectId, slot: i32) -> Option<i32> {
        let item = self.get(creature).and_then(|o| o.creature()).and_then(|c| c.equipped.get(&slot).copied())?;
        let base = self.get(item).and_then(|o| o.item()).map(|i| i.base_item)?;
        let t = self.tables.get(&self.res, "baseitems").ok()?;
        t.int(base as usize, "weaponwield").filter(|&w| w > 0)
    }

    /// Weapon wield class of the main-hand weapon (main hand, then creature
    /// weapons), 8 when unarmed.
    pub fn main_hand_wield(&self, creature: ObjectId) -> i32 {
        [SLOT_RIGHTWEAPON, SLOT_CWEAPON_R, SLOT_CWEAPON_B, SLOT_CWEAPON_L].iter().find_map(|&s| self.held_wield(creature, s)).unwrap_or(8)
    }

    /// Animation family the stance uses for the main-hand weapon (see
    /// combat::blaster_family): two pistols are the dual-blaster family.
    pub fn main_hand_family(&self, creature: ObjectId) -> i32 {
        // Off hand is slot 5 only, as in attack_animation: the creature weapon
        // slots are natural weapons (docs/notes/combat.md).
        crate::combat::blaster_family(self.main_hand_wield(creature), self.held_wield(creature, SLOT_LEFTWEAPON) == Some(4))
    }

    /// Switches the active weapon set (the X key): the pair in slots 4/5
    /// trades places with the pair in 18/19, so combat stats, held models
    /// and dual-wield rules follow it. False when neither set holds a
    /// weapon (nothing to switch to).
    pub fn swap_weapon_sets(&mut self, creature: ObjectId) -> bool {
        let Some(c) = self.get_mut(creature).and_then(|o| o.creature_mut()) else { return false };
        let pairs = [(SLOT_RIGHTWEAPON, SLOT_RIGHTWEAPON2), (SLOT_LEFTWEAPON, SLOT_LEFTWEAPON2)];
        if pairs.iter().all(|&(a, b)| !c.equipped.contains_key(&a) && !c.equipped.contains_key(&b)) {
            return false;
        }
        for (a, b) in pairs {
            let (x, y) = (c.equipped.remove(&a), c.equipped.remove(&b));
            if let Some(y) = y {
                c.equipped.insert(a, y);
            }
            if let Some(x) = x {
                c.equipped.insert(b, x);
            }
        }
        c.weapon_set ^= 1;
        let set = c.weapon_set;
        self.begin_draw(creature, SLOT_RIGHTWEAPON);
        self.emit(WorldEvent::EquipmentChanged { creature, slot: SLOT_RIGHTWEAPON });
        self.emit(WorldEvent::EquipmentChanged { creature, slot: SLOT_LEFTWEAPON });
        self.emit(WorldEvent::WeaponSetSwapped { creature, set });
        true
    }

    fn equipment_changed(&mut self, creature: ObjectId, slot: i32) {
        self.emit(WorldEvent::EquipmentChanged { creature, slot });
        if slot == SLOT_BODY {
            self.refresh_body(creature);
        }
    }

    /// Recomputes the body model from the worn armour (bodyvar letter and
    /// TextureVar), like the k2-data blueprint path does at spawn.
    fn refresh_body(&mut self, creature: ObjectId) {
        let Some((appearance, armour)) = self.get(creature).and_then(|o| o.creature()).map(|c| (c.appearance, c.equipped.get(&SLOT_BODY).copied())) else {
            return;
        };
        let (mut body_var, mut tex_var) = ('a', 1u32);
        if let Some(def) = armour.and_then(|a| self.item_def_of(a)) {
            if let Some(v) = self
                .tables
                .get(&self.res, "baseitems")
                .ok()
                .and_then(|t| t.str(def.base_item as usize, "bodyvar").and_then(|s| s.chars().next()))
            {
                body_var = v.to_ascii_lowercase();
            }
            tex_var = def.texture_variation.max(1);
        }
        let Ok(m) = self.tables.creature_models(&self.res, appearance, body_var, tex_var) else { return };
        if let Some(c) = self.get_mut(creature).and_then(|o| o.creature_mut()) {
            if c.body_model.as_deref() == Some(m.body.as_str()) && c.body_texture == m.body_texture {
                return;
            }
            c.body_model = Some(m.body);
            c.body_texture = m.body_texture;
        }
        self.emit(WorldEvent::AppearanceChanged { object: creature });
    }

    /// Re-derives every creature's body model and texture from its appearance
    /// and worn armour. Saves store the derived fields, so a save made before
    /// a body resolution fix would keep the old (wrong) look otherwise.
    pub(crate) fn refresh_all_bodies(&mut self) {
        let ids: Vec<ObjectId> = self.iter().filter(|o| o.creature().is_some()).map(|o| o.id).collect();
        for id in ids {
            self.refresh_body(id);
        }
    }

    /// ActionEquipMostDamagingMelee/Ranged: the best weapon the creature
    /// can reach (party inventory for party members) goes to the right hand.
    pub fn equip_best_weapon(&mut self, creature: ObjectId, ranged: bool) -> Option<ObjectId> {
        let mut best: Option<(f32, ObjectId)> = None;
        let mut cands = self.inventory_of(creature);
        cands.extend(self.equipment(creature).into_iter().filter(|&(s, _)| s == SLOT_RIGHTWEAPON).map(|(_, i)| i));
        for item in cands {
            let Some(def) = self.item_def_of(item) else { continue };
            if !def.fits(SLOT_RIGHTWEAPON) || self.can_equip(creature, item, SLOT_RIGHTWEAPON).is_err() {
                continue;
            }
            let Some(w) = self.item_info(item).and_then(|i| i.weapon(&self.res, &self.tables)) else { continue };
            if w.ranged != ranged {
                continue;
            }
            let avg = w.num_dice as f32 * (w.die as f32 + 1.0) / 2.0
                + w.extra_dice.iter().map(|&(n, d)| n as f32 * (d as f32 + 1.0) / 2.0).sum::<f32>()
                + w.damage_bonus as f32
                + w.attack_bonus as f32 * 0.5;
            if best.is_none_or(|(s, _)| avg > s) {
                best = Some((avg, item));
            }
        }
        let (_, item) = best?;
        let current = self.equipment(creature).into_iter().find(|&(s, _)| s == SLOT_RIGHTWEAPON).map(|(_, i)| i);
        if current != Some(item) {
            self.equip(creature, item, SLOT_RIGHTWEAPON).ok()?;
        }
        Some(item)
    }

    // ---------------------------------------------------------------
    // Containers
    // ---------------------------------------------------------------

    /// Whether using `target` opens a loot window.
    pub fn is_container(&self, target: ObjectId) -> bool {
        match self.get(target) {
            Some(o) if o.kind == ObjectKind::Placeable => o.placeable().is_some_and(|p| p.has_inventory),
            Some(o) if o.kind == ObjectKind::Creature => {
                o.creature().is_some_and(|c| c.dead) && !self.is_party_member(target) && !o.inventory.is_empty()
            }
            _ => false,
        }
    }

    /// The player opens a container (placeable with HasInventory, or a dead
    /// creature with loot). Locked placeables fire OnFailToOpen. Emits
    /// PlaceableChanged + ContainerOpened and fires OnOpen.
    pub fn open_container(&mut self, user: ObjectId, target: ObjectId) -> Result<Vec<ObjectId>, ItemError> {
        if !self.is_container(target) {
            return Err(ItemError::NotContainer);
        }
        let ctx = ScriptCtx { last_used_by: user, last_opened_by: user, clicking: user, ..Default::default() };
        let is_placeable = self.get(target).is_some_and(|o| o.kind == ObjectKind::Placeable);
        if is_placeable {
            if self.get(target).and_then(|o| o.placeable()).is_some_and(|p| p.locked) {
                self.fire(target, "OnFailToOpen", ctx);
                return Err(ItemError::Locked);
            }
            // A trapped container goes off as it opens (traps.rs).
            if self.get(target).and_then(|o| o.placeable()).is_some_and(|p| !p.open) {
                self.trigger_object_trap(target, user);
            }
            if let Some(p) = self.get_mut(target).and_then(|o| o.placeable_mut())
                && !p.open
            {
                p.open = true;
                self.emit(WorldEvent::PlaceableChanged { placeable: target, open: true });
            }
            self.fire(target, "OnOpen", ctx);
        }
        let items = self.inventory_of(target);
        self.inventory.open_container = Some(target);
        self.emit(WorldEvent::ContainerOpened { container: target, items: items.clone() });
        Ok(items)
    }

    pub fn container_views(&mut self, container: ObjectId) -> Vec<ItemView> {
        self.inventory_of(container).into_iter().filter_map(|i| self.item_view(i)).collect()
    }

    /// Moves one item from a container to the party; fires OnInvDisturbed.
    pub fn take_item(&mut self, container: ObjectId, item: ObjectId, taker: ObjectId) -> Result<ObjectId, ItemError> {
        if !self.inventory_of(container).contains(&item) {
            return Err(ItemError::NoItem);
        }
        let stack = self.get(item).and_then(|o| o.item()).map_or(1, |i| i.stack_size);
        let name = self.get(item).map(|o| strip_notes(&o.name)).unwrap_or_default();
        let held = self.store_item(item, taker);
        self.inventory.last_disturbed = Some(item);
        if self.is_party_member(taker) {
            self.emit(WorldEvent::ItemReceived { item: held, name, stack });
        }
        let ctx = ScriptCtx { last_used_by: taker, last_opened_by: taker, ..Default::default() };
        self.fire(container, "OnInvDisturbed", ctx);
        Ok(held)
    }

    /// Takes everything (also how a creature's loot is emptied).
    pub fn take_all(&mut self, container: ObjectId, taker: ObjectId) -> Vec<ObjectId> {
        let items = self.inventory_of(container);
        items.into_iter().filter_map(|i| self.take_item(container, i, taker).ok()).collect()
    }

    /// Closes the loot window: fires OnClosed and plays the close animation.
    pub fn close_container(&mut self, container: ObjectId, by: ObjectId) {
        if self.inventory.open_container == Some(container) {
            self.inventory.open_container = None;
        }
        if let Some(p) = self.get_mut(container).and_then(|o| o.placeable_mut())
            && p.open
        {
            p.open = false;
            self.emit(WorldEvent::PlaceableChanged { placeable: container, open: false });
            self.fire(container, "OnClosed", ScriptCtx { last_closed_by: by, ..Default::default() });
        }
        self.emit(WorldEvent::ContainerClosed { container });
    }

    // ---------------------------------------------------------------
    // Engine routines
    // ---------------------------------------------------------------

    /// Item, inventory and gold routines. Checked before the general
    /// dispatch so the party-inventory rules apply everywhere.
    pub(crate) fn item_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        let me_or = |n: usize| match a.get(n) {
            Some(Value::Object(o)) if *o != OBJECT_INVALID => *o,
            _ => me,
        };
        Some(match name {
            "CreateItemOnObject" => {
                // (sItemTemplate, oTarget = OBJECT_SELF, nStackSize = 1, nHideMessage = 0)
                let target = me_or(1);
                let stack = if a.len() > 2 { int(a, 2) } else { 1 };
                let quiet = int(a, 3) != 0;
                if !self.is_valid(target) && !self.is_party_member(target) {
                    return Some(Value::Object(OBJECT_INVALID));
                }
                o_opt(self.give_new_item(string(a, 0), target, stack, quiet))
            }
            "GetItemPossessedBy" => o_opt(self.find_possessed(obj(a, 0), string(a, 1))),
            "GetFirstItemInInventory" => {
                let t = me_or(0);
                let list = self.inventory_of(t);
                self.set_inventory_iter(t, list);
                Value::Object(self.next_inventory_item(t))
            }
            "GetNextItemInInventory" => Value::Object(self.next_inventory_item(me_or(0))),
            "ActionEquipItem" => {
                let (item, slot) = (obj(a, 0), int(a, 1));
                if let Err(e) = self.equip(me, item, slot) {
                    self.warnings.push(format!("ActionEquipItem {} slot {slot}: {e}", self.describe(item)));
                }
                Value::Void
            }
            "ActionUnequipItem" => {
                let item = obj(a, 0);
                if let Some((slot, _)) = self.equipment(me).into_iter().find(|&(_, i)| i == item) {
                    self.unequip(me, slot);
                }
                Value::Void
            }
            "ActionEquipMostDamagingMelee" | "ActionEquipMostDamagingRanged" => {
                self.equip_best_weapon(me, name.ends_with("Ranged"));
                Value::Void
            }
            "ActionTakeItem" => {
                // (oItem, oTakeFrom)
                let item = obj(a, 0);
                if self.is_valid(item) {
                    self.store_item(item, me);
                }
                Value::Void
            }
            "ActionGiveItem" | "GiveItem" => {
                // ActionGiveItem(oItem, oGiveTo); GiveItem(oItem, oGiveTo) in K2.
                let (item, to) = (obj(a, 0), obj(a, 1));
                if self.is_valid(item) {
                    if self.is_valid(to) || self.is_party_member(to) {
                        self.store_item(item, to);
                    } else {
                        self.detach_item(item);
                    }
                }
                Value::Void
            }
            "GetGold" => Value::Int(if self.is_party_member(me_or(0)) { self.credits() } else { 0 }),
            "GiveGoldToCreature" => {
                // (oCreature, nGP)
                if self.is_party_member(obj(a, 0)) {
                    self.add_credits(int(a, 1));
                }
                Value::Void
            }
            "TakeGoldFromCreature" => {
                // (nAmount, oCreatureToTakeFrom, bDestroy)
                if self.is_party_member(obj(a, 1)) {
                    self.add_credits(-int(a, 0));
                }
                Value::Void
            }
            "GetInventoryDisturbItem" => o_opt(self.inventory.last_disturbed),
            "OpenStore" => {
                // (oStore, oPC, nBonusMarkUp, nBonusMarkDown)
                let store = obj(a, 0);
                self.open_store(store, obj(a, 1), int(a, 2), int(a, 3));
                Value::Void
            }
            "DestroyObject" if self.get(obj(a, 0)).is_some_and(|o| o.kind == ObjectKind::Item) && a.get(1).is_none_or(|v| matches!(v, Value::Float(f) if *f <= 0.0)) => {
                let item = obj(a, 0);
                self.detach_item(item);
                self.destroy(item);
                Value::Void
            }
            _ => return None,
        })
    }

    fn set_inventory_iter(&mut self, owner: ObjectId, list: Vec<ObjectId>) {
        self.iters.inventory.insert(owner, (list, 0));
    }

    fn next_inventory_item(&mut self, owner: ObjectId) -> ObjectId {
        let Some((list, pos)) = self.iters.inventory.get_mut(&owner) else { return OBJECT_INVALID };
        let r = list.get(*pos).copied().unwrap_or(OBJECT_INVALID);
        *pos += 1;
        r
    }
}

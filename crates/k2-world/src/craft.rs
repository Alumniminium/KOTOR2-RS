//! Workbenches, lab stations and lock picking: item creation, breakdown,
//! upgrades and Security checks. Headless; the GUI drives these through the
//! World API below and learns when to open a screen from
//! [`WorldEvent::UpgradeScreen`] / [`WorldEvent::ItemCreateScreen`].
//!
//! How the shipped scripts get here (see docs/notes/crafting.md):
//! - Workbench placeables run `k_workbench` (OnUsed) or `a_compdlg` (opens
//!   the `workbnch` tutorial dialog, whose replies run `k_workbench_bd`,
//!   `k_workbench_bdm` or `k_workbench_t3`). All of them end in
//!   `ShowUpgradeScreen(item, character, nDisableItemCreation,
//!   nDisableUpgrade, sOverride2DA)`.
//! - Lab stations run `a_compdlg` into `000_medstation`, whose reply runs
//!   `k_medstation`: `ShowChemicalUpgradeScreen(GetLastUsedBy())`.
//!
//! Recipes come from itemcreate.2da (or the override 2DA a script names,
//! e.g. itemcreatemira) and chemicalcreate.2da. COMPONENTS and CHEMICALS are
//! ordinary stackable items (compont_00001 / chem_00001, tags "Components" /
//! "Chemicals") in the party inventory.
//!
//! Upgrades: upgrade.2da maps upgrade templates to upgradetypes.2da rows,
//! upcrystals.2da lists lightsaber colour crystals. An upgrade fits a slot
//! when its type matches and its UpgradeLevel is at most the item's
//! UpgradeLevel (inferred from the shipped data; see the notes).
//!
//! Item value -> COMPONENT/CHEMICAL counts and the breakdown yield are not
//! in any table; the engine computes them. The formulas here are documented
//! approximations, isolated in [`unit_cost`] and [`breakdown_yield`].

use std::collections::HashMap;

use k2_data::GffExt;
use k2_script::{ObjectId, Value, OBJECT_INVALID};
use rakata_core::ResourceType;

use crate::object::{Detail, ObjectKind};
use crate::{ScriptCtx, World, WorldEvent};
use serde::{Deserialize, Serialize};

pub const SKILL_COMPUTER_USE: usize = 0;
pub const SKILL_DEMOLITIONS: usize = 1;
pub const SKILL_STEALTH: usize = 2;
pub const SKILL_AWARENESS: usize = 3;
pub const SKILL_PERSUADE: usize = 4;
pub const SKILL_REPAIR: usize = 5;
pub const SKILL_SECURITY: usize = 6;
pub const SKILL_TREAT_INJURY: usize = 7;
pub const SKILL_COUNT: usize = 8;

pub const COMPONENTS_TEMPLATE: &str = "compont_00001";
pub const CHEMICALS_TEMPLATE: &str = "chem_00001";
const COMPONENTS_TAG: &str = "Components";
const CHEMICALS_TAG: &str = "Chemicals";

/// Skill ranks for a PC with no character generation yet.
pub const DEFAULT_PC_SKILL_RANK: i32 = 4;

/// upgradetypes.2da rows; `COLOR_CRYSTAL` is the lightsaber colour slot,
/// filled from upcrystals.2da rather than upgrade.2da.
pub const UPGRADE_SABER_CRYSTAL: i32 = 0;
pub const UPGRADE_SABER_LENS: i32 = 1;
pub const UPGRADE_SABER_EMITTER: i32 = 2;
pub const UPGRADE_SABER_CELL: i32 = 3;
pub const UPGRADE_TARGETING: i32 = 4;
pub const UPGRADE_POWER_PACK: i32 = 5;
pub const UPGRADE_FIRING_CHAMBER: i32 = 6;
pub const UPGRADE_MELEE_GRIP: i32 = 7;
pub const UPGRADE_MELEE_BLADE: i32 = 8;
pub const UPGRADE_MELEE_CELL: i32 = 9;
pub const UPGRADE_ARMOR_OVERLAY: i32 = 10;
pub const UPGRADE_ARMOR_UNDERLAY: i32 = 11;
pub const COLOR_CRYSTAL: i32 = -1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Station {
    Workbench,
    LabStation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Currency {
    Components,
    Chemicals,
}

impl Currency {
    pub fn template(self) -> &'static str {
        match self {
            Currency::Components => COMPONENTS_TEMPLATE,
            Currency::Chemicals => CHEMICALS_TEMPLATE,
        }
    }

    fn tag(self) -> &'static str {
        match self {
            Currency::Components => COMPONENTS_TAG,
            Currency::Chemicals => CHEMICALS_TAG,
        }
    }
}

/// What a script asked the GUI to show.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CraftScreen {
    pub station: Station,
    /// Forced upgrade target (ShowUpgradeScreen oItem), if any.
    pub item: Option<ObjectId>,
    /// Whose skills count.
    pub character: ObjectId,
    pub item_creation: bool,
    pub upgrade: bool,
    /// Recipe table: itemcreate, an override (itemcreatemira, ...) or
    /// chemicalcreate.
    pub recipes: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    /// Row in the recipe table.
    pub row: usize,
    pub template: String,
    pub name: String,
    /// Tab in the create screen (the table's group column).
    pub group: i32,
    /// SKILL_* the recipe needs.
    pub skill: usize,
    pub rank_required: i32,
    /// Minimum character level (itemcreate `level`; 0 = none).
    pub level_required: i32,
    pub currency: Currency,
    pub cost: i32,
    /// Tag the created item gets (itemcreate `overridetag`), for plot checks.
    pub override_tag: Option<String>,
    /// The character's rank is high enough.
    pub skill_ok: bool,
    /// The party holds enough components/chemicals.
    pub affordable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct UpgradeSlot {
    pub slot: usize,
    /// upgradetypes.2da row, or [`COLOR_CRYSTAL`].
    pub ty: i32,
    pub label: String,
    /// Template of the installed upgrade.
    pub installed: Option<String>,
    /// Party inventory items that fit this slot now.
    pub candidates: Vec<ObjectId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LockInfo {
    pub locked: bool,
    pub dc: i32,
    pub key_required: bool,
    pub key_tag: String,
    pub has_key: bool,
    pub security_rank: i32,
    /// Out of combat a check takes 20; this is whether that succeeds.
    pub can_pick: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CraftError {
    NoSuchRecipe,
    SkillTooLow { needed: i32, have: i32 },
    LevelTooLow { needed: i32, have: i32 },
    NotEnough { currency: &'static str, needed: i32, have: i32 },
    NotInInventory,
    CannotBreakDown(&'static str),
    NotUpgradable,
    NoSuchSlot,
    DoesNotFit,
    SlotEmpty,
    NotLocked,
    KeyRequired,
    CheckFailed { dc: i32, total: i32 },
    CreateFailed,
}

impl std::fmt::Display for CraftError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Lightsaber,
    Ranged,
    Melee,
    Armor,
}

/// Per-world crafting state.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct CraftState {
    /// The screen the last script opened; the GUI closes it with
    /// [`World::close_craft_screen`].
    #[serde(skip)]
    pub screen: Option<CraftScreen>,
    /// Explicit ranks (character generation, level up, tests).
    pub skills: HashMap<ObjectId, [i32; SKILL_COUNT]>,
    #[serde(skip)]
    template_skills: HashMap<String, [i32; SKILL_COUNT]>,
    /// Item object -> installed upgrade template per slot index.
    pub(crate) installed: HashMap<ObjectId, Vec<Option<String>>>,
    /// Lightsaber colour crystal model overrides (upcrystals.2da).
    pub model_override: HashMap<ObjectId, String>,
    #[serde(skip)]
    upgrade_types: Option<HashMap<String, i32>>,
    rng: u64,
}

impl CraftState {
    fn d20(&mut self) -> i32 {
        if self.rng == 0 {
            self.rng = 0x9E37_79B9_7F4A_7C15;
        }
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        ((x.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) % 20) as i32 + 1
    }
}

/// COMPONENTS/CHEMICALS an item is worth: one per 10 credits of value,
/// at least one. Approximation; the engine's formula is not in the data.
pub fn unit_cost(value: i32) -> i32 {
    ((value.max(0) + 9) / 10).max(1)
}

/// What breaking an item down returns: a quarter of its unit cost, plus
/// 2.5% per rank of the relevant skill, capped at the full cost.
/// Approximation of "depends on the item's complexity and value and the
/// character's skill" (dialog.tlk 76291).
pub fn breakdown_yield(value: i32, rank: i32) -> i32 {
    let units = unit_cost(value) as f32;
    let share = (0.25 + 0.025 * rank.max(0) as f32).min(1.0);
    ((units * share).floor() as i32).max(1)
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

impl World {
    // ---------------------------------------------------------------
    // Engine routines
    // ---------------------------------------------------------------

    /// Crafting, skill and lock routines. None = not handled here.
    pub(crate) fn craft_call(&mut self, name: &str, me: ObjectId, a: &[Value]) -> Option<Value> {
        let v = match name {
            "ShowUpgradeScreen" => {
                let item = Some(obj(a, 0)).filter(|&i| self.is_valid(i));
                let character = Some(obj(a, 1)).filter(|&c| self.is_valid(c)).unwrap_or_else(|| self.party_leader());
                let item_creation = int(a, 2) == 0;
                let upgrade = int(a, 3) == 0;
                let table = string(a, 4).trim().to_ascii_lowercase();
                let recipes = if table.is_empty() { "itemcreate".to_owned() } else { table };
                self.open_craft_screen(CraftScreen { station: Station::Workbench, item, character, item_creation, upgrade, recipes });
                Value::Void
            }
            "ShowChemicalUpgradeScreen" => {
                let character = Some(obj(a, 0)).filter(|&c| self.is_valid(c)).unwrap_or_else(|| self.party_leader());
                self.open_craft_screen(CraftScreen {
                    station: Station::LabStation,
                    item: None,
                    character,
                    item_creation: true,
                    upgrade: false,
                    recipes: "chemicalcreate".to_owned(),
                });
                Value::Void
            }
            "GetItemComponent" | "GetItemComponentPieceValue" => Value::Int(self.currency_count(Currency::Components)),
            "GetChemicals" | "GetChemicalPieceValue" => Value::Int(self.currency_count(Currency::Chemicals)),
            "GetSkillRank" | "GetSkillRankBase" => {
                let skill = int(a, 0);
                let target = if a.len() > 1 { obj(a, 1) } else { me };
                let target = if target == OBJECT_INVALID && a.len() <= 1 { me } else { target };
                match usize::try_from(skill).ok().filter(|&s| s < SKILL_COUNT) {
                    Some(s) if self.get(target).is_some_and(|o| o.kind == ObjectKind::Creature) => Value::Int(self.skill_rank(target, s)),
                    // Engine returns -1 for an invalid skill or non-creature.
                    _ => Value::Int(-1),
                }
            }
            "GetHasSkill" => {
                let skill = int(a, 0);
                let target = if a.len() > 1 && obj(a, 1) != OBJECT_INVALID { obj(a, 1) } else { me };
                let has = usize::try_from(skill).ok().filter(|&s| s < SKILL_COUNT).is_some_and(|s| self.skill_rank(target, s) > 0);
                Value::Int(i32::from(has))
            }
            "GetLockUnlockDC" => Value::Int(self.lock_dc(obj(a, 0))),
            // Only Security here: the other skills fall through to traps.rs
            // (Demolitions on traps) and stealth.rs (Stealth). GetIsTrapped
            // lives in traps.rs.
            "ActionUseSkill" if usize::try_from(int(a, 0)).ok() == Some(SKILL_SECURITY) => {
                // ActionUseSkill(nSkill, oTarget, nSubSkill, oItemUsed)
                let target = obj(a, 1);
                if self.is_valid(target) {
                    let _ = self.try_unlock(me, target);
                }
                Value::Void
            }
            _ => return None,
        };
        Some(v)
    }

    // ---------------------------------------------------------------
    // Screens
    // ---------------------------------------------------------------

    fn open_craft_screen(&mut self, screen: CraftScreen) {
        if screen.station == Station::Workbench && screen.upgrade {
            self.emit(WorldEvent::UpgradeScreen {
                item: screen.item,
                character: screen.character,
                item_creation: screen.item_creation,
                recipes: screen.recipes.clone(),
            });
        } else {
            self.emit(WorldEvent::ItemCreateScreen { station: screen.station, character: screen.character, recipes: screen.recipes.clone() });
        }
        self.craft.screen = Some(screen);
    }

    /// The GUI closed the workbench/lab screen.
    pub fn close_craft_screen(&mut self) {
        self.craft.screen = None;
    }

    // ---------------------------------------------------------------
    // Skills
    // ---------------------------------------------------------------

    /// A creature's rank in SKILL_*: explicit ranks first, then the UTC
    /// SkillList of its template, then [`DEFAULT_PC_SKILL_RANK`] for the PC.
    pub fn skill_rank(&mut self, id: ObjectId, skill: usize) -> i32 {
        if skill >= SKILL_COUNT {
            return -1;
        }
        if let Some(r) = self.craft.skills.get(&id) {
            return r[skill];
        }
        let Some(o) = self.get(id) else { return 0 };
        let is_pc = o.creature().is_some_and(|c| c.is_pc);
        let template = o.template.clone();
        if template.is_empty() {
            return if is_pc { DEFAULT_PC_SKILL_RANK } else { 0 };
        }
        if let Some(r) = self.craft.template_skills.get(&template) {
            return r[skill];
        }
        let mut ranks = [0; SKILL_COUNT];
        if let Ok(g) = k2_data::read_gff(&self.res, &template, ResourceType::Utc) {
            for (i, s) in g.root.list("SkillList").iter().take(SKILL_COUNT).enumerate() {
                ranks[i] = s.int("Rank").unwrap_or(0);
            }
        }
        self.craft.template_skills.insert(template, ranks);
        ranks[skill]
    }

    pub fn set_skill_rank(&mut self, id: ObjectId, skill: usize, rank: i32) {
        if skill >= SKILL_COUNT {
            return;
        }
        let mut ranks = [0; SKILL_COUNT];
        for (s, r) in ranks.iter_mut().enumerate() {
            *r = self.skill_rank(id, s);
        }
        ranks[skill] = rank;
        self.craft.skills.insert(id, ranks);
    }

    fn character_level(&self, id: ObjectId) -> i32 {
        self.get(id).and_then(|o| o.creature()).map_or(1, |c| c.classes.iter().map(|&(_, l)| l).sum::<i32>().max(1))
    }

    // ---------------------------------------------------------------
    // Currency
    // ---------------------------------------------------------------

    pub fn currency_count(&self, currency: Currency) -> i32 {
        self.party_inventory()
            .into_iter()
            .filter(|&i| self.get(i).is_some_and(|o| o.tag.eq_ignore_ascii_case(currency.tag()) || o.template == currency.template()))
            .filter_map(|i| self.get(i).and_then(|o| o.item()).map(|s| s.stack_size))
            .sum()
    }

    fn spend_currency(&mut self, currency: Currency, mut amount: i32) -> bool {
        if self.currency_count(currency) < amount {
            return false;
        }
        let stacks: Vec<ObjectId> = self
            .party_inventory()
            .into_iter()
            .filter(|&i| self.get(i).is_some_and(|o| o.tag.eq_ignore_ascii_case(currency.tag()) || o.template == currency.template()))
            .collect();
        for s in stacks {
            if amount == 0 {
                break;
            }
            let have = self.get(s).and_then(|o| o.item()).map_or(0, |i| i.stack_size);
            let take = have.min(amount);
            amount -= take;
            self.take_units(s, take);
        }
        true
    }

    /// Removes `units` from an item stack, destroying it when empty.
    fn take_units(&mut self, item: ObjectId, units: i32) {
        let left = {
            let Some(it) = self.get_mut(item).and_then(|o| o.item_mut()) else { return };
            it.stack_size -= units;
            it.stack_size
        };
        if left <= 0 {
            self.detach_item(item);
            if let Some(o) = self.get_mut(item) {
                o.destroyed = true;
            }
            self.craft.installed.remove(&item);
            self.craft.model_override.remove(&item);
        }
    }

    // ---------------------------------------------------------------
    // Item creation and breakdown
    // ---------------------------------------------------------------

    /// Recipes of a table ("itemcreate", "itemcreatemira", "chemicalcreate",
    /// "tutitemcreate"), evaluated for `character`.
    pub fn recipes(&mut self, table: &str, character: ObjectId) -> Vec<Recipe> {
        let Ok(t) = self.tables.get(&self.res, table) else { return Vec::new() };
        let chemical = t.has_column("skilllevel");
        let currency = if chemical { Currency::Chemicals } else { Currency::Components };
        let have = self.currency_count(currency);
        let level = self.character_level(character);
        let mut out = Vec::new();
        for row in 0..t.rows() {
            let Some(template) = t.row_label(row).map(str::to_ascii_lowercase).or_else(|| t.resref(row, "label")) else { continue };
            let template = t.resref(row, "label").unwrap_or(template);
            let Some(def) = self.item_def(&template) else { continue };
            let rank_required = t.int(row, if chemical { "skilllevel" } else { "skill" }).unwrap_or(0);
            let skill = t.int(row, "base_skill").unwrap_or(SKILL_REPAIR as i32).clamp(0, SKILL_COUNT as i32 - 1) as usize;
            let cost = unit_cost(def.cost);
            let rank = self.skill_rank(character, skill);
            let level_required = t.int(row, "level").unwrap_or(0);
            out.push(Recipe {
                row,
                template: template.clone(),
                name: def.name.clone(),
                group: t.int(row, "group").unwrap_or(0),
                skill,
                rank_required,
                level_required,
                currency,
                cost,
                override_tag: t.str(row, "overridetag").filter(|s| !s.is_empty() && *s != "****").map(str::to_owned),
                skill_ok: rank >= rank_required && level >= level_required,
                affordable: have >= cost,
            });
        }
        out
    }

    /// Creates the recipe's item into the party inventory, spending the
    /// components or chemicals.
    pub fn create_item_from_recipe(&mut self, table: &str, row: usize, character: ObjectId) -> Result<ObjectId, CraftError> {
        let recipe = self.recipes(table, character).into_iter().find(|r| r.row == row).ok_or(CraftError::NoSuchRecipe)?;
        let rank = self.skill_rank(character, recipe.skill);
        if rank < recipe.rank_required {
            return Err(CraftError::SkillTooLow { needed: recipe.rank_required, have: rank });
        }
        let level = self.character_level(character);
        if level < recipe.level_required {
            return Err(CraftError::LevelTooLow { needed: recipe.level_required, have: level });
        }
        let have = self.currency_count(recipe.currency);
        if have < recipe.cost {
            let currency = if recipe.currency == Currency::Components { "components" } else { "chemicals" };
            return Err(CraftError::NotEnough { currency, needed: recipe.cost, have });
        }
        let leader = self.party_leader();
        let item = self.give_new_item(&recipe.template, leader, 1, false).ok_or(CraftError::CreateFailed)?;
        self.spend_currency(recipe.currency, recipe.cost);
        if let Some(tag) = recipe.override_tag
            && let Some(o) = self.get_mut(item)
        {
            o.tag = tag;
        }
        Ok(item)
    }

    /// Whether `item` belongs to the lab station (it is in chemicalcreate).
    fn is_chemical_item(&mut self, template: &str) -> bool {
        self.tables
            .get(&self.res, "chemicalcreate")
            .ok()
            .is_some_and(|t| (0..t.rows()).any(|r| t.resref(r, "label").is_some_and(|l| l.eq_ignore_ascii_case(template))))
    }

    /// The skill that governs breaking `template` down at `station`.
    fn breakdown_skill(&mut self, template: &str, station: Station) -> usize {
        if station == Station::LabStation {
            return SKILL_TREAT_INJURY;
        }
        self.tables
            .get(&self.res, "itemcreate")
            .ok()
            .and_then(|t| {
                (0..t.rows())
                    .find(|&r| t.resref(r, "label").is_some_and(|l| l.eq_ignore_ascii_case(template)))
                    .and_then(|r| t.int(r, "base_skill"))
            })
            .map_or(SKILL_REPAIR, |s| s.clamp(0, SKILL_COUNT as i32 - 1) as usize)
    }

    /// What breaking down one unit of `item` would give, without doing it.
    pub fn breakdown_preview(&mut self, item: ObjectId, station: Station, character: ObjectId) -> Result<(Currency, i32), CraftError> {
        let o = self.get(item).filter(|o| o.kind == ObjectKind::Item).ok_or(CraftError::NotInInventory)?;
        let template = o.template.clone();
        let tag = o.tag.clone();
        let plot = o.plot || o.item().is_some_and(|i| i.plot);
        if !self.party_inventory().contains(&item) {
            return Err(CraftError::NotInInventory);
        }
        if plot {
            return Err(CraftError::CannotBreakDown("plot item"));
        }
        if tag.eq_ignore_ascii_case(COMPONENTS_TAG) || tag.eq_ignore_ascii_case(CHEMICALS_TAG) {
            return Err(CraftError::CannotBreakDown("raw material"));
        }
        let chemical = self.is_chemical_item(&template);
        let currency = match (station, chemical) {
            (Station::LabStation, true) => Currency::Chemicals,
            (Station::Workbench, false) => Currency::Components,
            (Station::LabStation, false) => return Err(CraftError::CannotBreakDown("not a medical or chemical item")),
            (Station::Workbench, true) => return Err(CraftError::CannotBreakDown("medical or chemical item")),
        };
        let value = self.item_def(&template).map_or(0, |d| d.cost);
        let skill = self.breakdown_skill(&template, station);
        let rank = self.skill_rank(character, skill);
        Ok((currency, breakdown_yield(value, rank)))
    }

    /// Breaks down one unit of `item` into components or chemicals.
    pub fn break_down(&mut self, item: ObjectId, station: Station, character: ObjectId) -> Result<(Currency, i32), CraftError> {
        let (currency, amount) = self.breakdown_preview(item, station, character)?;
        let upgrades: Vec<String> = self.craft.installed.get(&item).map(|v| v.iter().flatten().cloned().collect()).unwrap_or_default();
        self.take_units(item, 1);
        // Installed upgrades come back rather than vanishing with the item.
        let leader = self.party_leader();
        for u in upgrades {
            self.give_new_item(&u, leader, 1, true);
        }
        self.give_new_item(currency.template(), leader, amount, false);
        Ok((currency, amount))
    }

    // ---------------------------------------------------------------
    // Upgrades
    // ---------------------------------------------------------------

    fn upgrade_type_of(&mut self, template: &str) -> Option<i32> {
        if self.craft.upgrade_types.is_none() {
            let mut m = HashMap::new();
            if let Ok(t) = self.tables.get(&self.res, "upgrade") {
                for r in 0..t.rows() {
                    if let (Some(tpl), Some(ty)) = (t.resref(r, "template"), t.int(r, "upgradetype")) {
                        m.insert(tpl.to_ascii_lowercase(), ty);
                    }
                }
            }
            if let Ok(t) = self.tables.get(&self.res, "upcrystals") {
                for r in 0..t.rows() {
                    if let Some(tpl) = t.resref(r, "template") {
                        m.insert(tpl.to_ascii_lowercase(), COLOR_CRYSTAL);
                    }
                }
            }
            self.craft.upgrade_types = Some(m);
        }
        self.craft.upgrade_types.as_ref().and_then(|m| m.get(&template.to_ascii_lowercase()).copied())
    }

    fn upgrade_kind(&mut self, item: ObjectId) -> Option<Kind> {
        let def = self.item_def_of(item)?;
        let t = self.tables.get(&self.res, "baseitems").ok()?;
        let row = def.base_item as usize;
        let class = t.str(row, "itemclass").unwrap_or("").to_ascii_lowercase();
        if matches!(class.as_str(), "w_lghtsbr" | "w_dblsbr" | "w_shortsbr") {
            return Some(Kind::Lightsaber);
        }
        // Creature weapons (w_cr*) and droid plating are not upgradable.
        if class.starts_with("w_cr") {
            return None;
        }
        if t.int(row, "rangedweapon") == Some(1) {
            return Some(Kind::Ranged);
        }
        if t.int(row, "weapontype").is_some() && class.starts_with("w_") {
            return Some(Kind::Melee);
        }
        // Body armour and robes: armortype column plus the body slot.
        if t.str(row, "armortype").is_some_and(|s| !s.is_empty() && s != "****") && def.equip_slots & 0x2 != 0 && !class.starts_with("i_drd") {
            return Some(Kind::Armor);
        }
        None
    }

    fn slot_types(kind: Kind) -> &'static [(i32, &'static str)] {
        match kind {
            Kind::Lightsaber => &[
                (COLOR_CRYSTAL, "Color Crystal"),
                (UPGRADE_SABER_CRYSTAL, "Power Crystal"),
                (UPGRADE_SABER_LENS, "Lens"),
                (UPGRADE_SABER_EMITTER, "Emitter"),
                (UPGRADE_SABER_CELL, "Energy Cell"),
            ],
            Kind::Ranged => &[(UPGRADE_TARGETING, "Targeting"), (UPGRADE_FIRING_CHAMBER, "Firing Chamber"), (UPGRADE_POWER_PACK, "Power Pack")],
            Kind::Melee => &[(UPGRADE_MELEE_GRIP, "Grip"), (UPGRADE_MELEE_BLADE, "Edge"), (UPGRADE_MELEE_CELL, "Energy Cell")],
            Kind::Armor => &[(UPGRADE_ARMOR_OVERLAY, "Overlay"), (UPGRADE_ARMOR_UNDERLAY, "Underlay")],
        }
    }

    /// Whether `upgrade` (an item) fits slot type `ty` of `item`.
    fn fits(&mut self, item: ObjectId, ty: i32, upgrade: ObjectId) -> bool {
        let Some(tpl) = self.get(upgrade).filter(|o| o.kind == ObjectKind::Item).map(|o| o.template.clone()) else { return false };
        if self.upgrade_type_of(&tpl) != Some(ty) {
            return false;
        }
        let item_level = self.item_def_of(item).map_or(0, |d| d.upgrade_level);
        let up_level = self.item_def(&tpl).map_or(0, |d| d.upgrade_level);
        up_level <= item_level
    }

    /// Upgrade slots of a weapon or armour, with what is installed and which
    /// party items would fit. Empty for items that take no upgrades.
    pub fn upgrade_slots(&mut self, item: ObjectId) -> Vec<UpgradeSlot> {
        let Some(kind) = self.upgrade_kind(item) else { return Vec::new() };
        let installed = self.craft.installed.get(&item).cloned().unwrap_or_default();
        let inventory = self.party_inventory();
        let mut out = Vec::new();
        for (slot, &(ty, label)) in Self::slot_types(kind).iter().enumerate() {
            let candidates = inventory.iter().copied().filter(|&u| u != item && self.fits(item, ty, u)).collect();
            out.push(UpgradeSlot { slot, ty, label: label.to_owned(), installed: installed.get(slot).cloned().flatten(), candidates });
        }
        out
    }

    /// Installs `upgrade` (one unit, from the party inventory) into `slot`,
    /// returning any upgrade it replaces to the inventory.
    /// Returns the upgraded item: `item` itself, or the unit split off when
    /// `item` was a stack.
    pub fn apply_upgrade(&mut self, item: ObjectId, slot: usize, upgrade: ObjectId) -> Result<ObjectId, CraftError> {
        let kind = self.upgrade_kind(item).ok_or(CraftError::NotUpgradable)?;
        let &(ty, _) = Self::slot_types(kind).get(slot).ok_or(CraftError::NoSuchSlot)?;
        if !self.party_inventory().contains(&upgrade) {
            return Err(CraftError::NotInInventory);
        }
        if !self.fits(item, ty, upgrade) {
            return Err(CraftError::DoesNotFit);
        }
        if self.craft.installed.get(&item).and_then(|v| v.get(slot).cloned().flatten()).is_some() {
            self.remove_upgrade(item, slot)?;
        }
        let tpl = self.get(upgrade).map(|o| o.template.clone()).unwrap_or_default();
        self.take_units(upgrade, 1);
        let item = self.split_one(item);
        let slots = self.craft.installed.entry(item).or_default();
        if slots.len() <= slot {
            slots.resize(slot + 1, None);
        }
        slots[slot] = Some(tpl.clone());
        if ty == COLOR_CRYSTAL {
            self.set_crystal_model(item, kind, Some(&tpl));
        }
        self.after_upgrade_change(item);
        Ok(item)
    }

    /// For a stack, splits one unit off as its own object (upgrades apply
    /// to a single item); returns the item to upgrade.
    fn split_one(&mut self, item: ObjectId) -> ObjectId {
        let units = self.get(item).and_then(|o| o.item()).map_or(1, |i| i.stack_size);
        if units <= 1 {
            return item;
        }
        let Some(o) = self.get(item) else { return item };
        let (template, tag) = (o.template.clone(), o.tag.clone());
        let holder = o.item().and_then(|i| i.possessor).unwrap_or_else(|| self.party_leader());
        let Some(one) = self.create_item(&template, OBJECT_INVALID, 1) else { return item };
        if let Some(it) = self.get_mut(one).and_then(|o| o.item_mut()) {
            it.stack_size = 1;
        }
        if let Some(o) = self.get_mut(one) {
            o.tag = tag;
        }
        // Mark it unique before storing so it does not merge straight back.
        self.craft.installed.insert(one, Vec::new());
        self.store_item(one, holder);
        self.take_units(item, 1);
        one
    }

    /// Takes the upgrade out of `slot` and puts it back in the party
    /// inventory.
    pub fn remove_upgrade(&mut self, item: ObjectId, slot: usize) -> Result<ObjectId, CraftError> {
        let kind = self.upgrade_kind(item).ok_or(CraftError::NotUpgradable)?;
        let tpl = self.craft.installed.get_mut(&item).and_then(|v| v.get_mut(slot)).and_then(Option::take).ok_or(CraftError::SlotEmpty)?;
        if Self::slot_types(kind).get(slot).is_some_and(|&(ty, _)| ty == COLOR_CRYSTAL) {
            self.set_crystal_model(item, kind, None);
        }
        let leader = self.party_leader();
        let back = self.give_new_item(&tpl, leader, 1, true).ok_or(CraftError::CreateFailed)?;
        self.after_upgrade_change(item);
        Ok(back)
    }

    /// Installed upgrade templates of an item (slot order, None = empty).
    pub fn installed_upgrades(&self, item: ObjectId) -> Vec<Option<String>> {
        self.craft.installed.get(&item).cloned().unwrap_or_default()
    }

    /// Model for a lightsaber whose colour crystal was changed.
    pub fn item_model_override(&self, item: ObjectId) -> Option<&str> {
        self.craft.model_override.get(&item).map(String::as_str)
    }

    fn set_crystal_model(&mut self, item: ObjectId, kind: Kind, crystal: Option<&str>) {
        if kind != Kind::Lightsaber {
            return;
        }
        let Some(crystal) = crystal else {
            self.craft.model_override.remove(&item);
            return;
        };
        let class = self
            .item_def_of(item)
            .map(|d| d.item_class.to_ascii_lowercase())
            .unwrap_or_default();
        let column = match class.as_str() {
            "w_shortsbr" => "shortmdlvar",
            "w_dblsbr" => "doublemdlvar",
            _ => "longmdlvar",
        };
        let model = self.tables.get(&self.res, "upcrystals").ok().and_then(|t| {
            (0..t.rows())
                .find(|&r| t.resref(r, "template").is_some_and(|x| x.eq_ignore_ascii_case(crystal)))
                .and_then(|r| t.resref(r, column))
        });
        match model {
            Some(m) => {
                self.craft.model_override.insert(item, m.to_ascii_lowercase());
            }
            None => {
                self.craft.model_override.remove(&item);
            }
        }
    }

    /// Re-announces an equipped item so held models and stats refresh.
    fn after_upgrade_change(&mut self, item: ObjectId) {
        let holder = self.get(item).and_then(|o| o.item()).and_then(|i| i.possessor);
        if let Some(h) = holder
            && let Some(slot) = self.get(h).and_then(|o| o.creature()).and_then(|c| c.equipped.iter().find(|&(_, &i)| i == item).map(|(&s, _)| s))
        {
            self.emit(WorldEvent::EquipmentChanged { creature: h, slot });
        }
    }

    /// Item properties the installed upgrades add (merged into the item's
    /// own list by `item_info`).
    pub(crate) fn upgrade_properties(&self, item: ObjectId) -> Vec<crate::items::ItemProperty> {
        let Some(slots) = self.craft.installed.get(&item) else { return Vec::new() };
        slots
            .iter()
            .flatten()
            .filter_map(|tpl| crate::items::ItemInfo::load(&self.res, tpl))
            .flat_map(|i| i.properties)
            .collect()
    }

    // ---------------------------------------------------------------
    // Locks and Security
    // ---------------------------------------------------------------

    fn lock_dc(&self, target: ObjectId) -> i32 {
        let Some(o) = self.get(target) else { return 0 };
        match &o.detail {
            Detail::Door(_) => k2_data::Door::load(&self.res, &o.template).map_or(0, |d| d.open_lock_dc),
            Detail::Placeable(_) => k2_data::Placeable::load(&self.res, &o.template).map_or(0, |p| p.open_lock_dc),
            _ => 0,
        }
    }

    fn is_locked(&self, target: ObjectId) -> bool {
        self.get(target).is_some_and(|o| match &o.detail {
            Detail::Door(d) => d.locked,
            Detail::Placeable(p) => p.locked,
            _ => false,
        })
    }

    /// What the GUI shows for a locked door or container.
    pub fn lock_info(&mut self, user: ObjectId, target: ObjectId) -> Option<LockInfo> {
        let o = self.get(target)?;
        let (key_required, key_tag) = match &o.detail {
            Detail::Door(d) => (d.key_required, d.key_name.clone()),
            Detail::Placeable(_) => k2_data::Placeable::load(&self.res, &o.template).map_or((false, String::new()), |p| (p.key_required, p.key_name)),
            _ => return None,
        };
        let locked = self.is_locked(target);
        let dc = self.lock_dc(target);
        let has_key = !key_tag.is_empty() && self.find_possessed(user, &key_tag).is_some();
        let security_rank = self.skill_rank(user, SKILL_SECURITY);
        let can_pick = !key_required && security_rank + 20 >= dc;
        Some(LockInfo { locked, dc, key_required, key_tag, has_key, security_rank, can_pick })
    }

    /// Opens a lock with a key or the Security skill: out of combat the
    /// check takes 20, in combat it rolls d20. Success unlocks, fires
    /// OnUnlock, then opens the door or container at once (a static door
    /// talks instead, and a linked door transits).
    pub fn try_unlock(&mut self, user: ObjectId, target: ObjectId) -> Result<(), CraftError> {
        let info = self.lock_info(user, target).ok_or(CraftError::NotLocked)?;
        if !info.locked {
            return Err(CraftError::NotLocked);
        }
        let (success, total) = if info.has_key {
            (true, 0)
        } else if info.key_required {
            return Err(CraftError::KeyRequired);
        } else {
            let roll = if self.in_combat(user) { self.craft.d20() } else { 20 };
            let total = info.security_rank + roll;
            (total >= info.dc, total)
        };
        self.emit(WorldEvent::SkillCheck { user, target, skill: SKILL_SECURITY as i32, dc: info.dc, total, success });
        if !success {
            let ctx = ScriptCtx { last_opened_by: user, clicking: user, ..Default::default() };
            self.fire(target, "OnFailToOpen", ctx);
            return Err(CraftError::CheckFailed { dc: info.dc, total });
        }
        self.set_locked(target, false);
        let ctx = ScriptCtx { last_used_by: user, last_opened_by: user, clicking: user, ..Default::default() };
        self.fire(target, "OnUnlock", ctx);
        // The unlock is a use of the object: it opens at once, with no line, since
        // the lock's line is gone. A static door never opens, so it talks as
        // use_object does. A linked door goes through use_object (its OnClick, then
        // the transit).
        match self.get(target).map(|o| o.kind) {
            Some(ObjectKind::Door) if self.get(target).and_then(|o| o.door()).is_some_and(|d| d.linked_to_module.is_none()) => {
                let click = ScriptCtx { last_used_by: user, clicking: user, last_opened_by: user, last_speaker: user, ..Default::default() };
                self.fire(target, "OnClick", click);
                let (is_static, open) = self.get(target).and_then(|o| o.door()).map_or((false, false), |d| (d.is_static, d.open));
                if is_static {
                    let _ = self.talk_on_use(target, user);
                } else if !open {
                    self.trigger_object_trap(target, user);
                    self.set_door_open(target, true, user);
                }
            }
            Some(ObjectKind::Placeable) if self.is_container(target) => {
                let _ = self.open_container(user, target);
            }
            _ => self.use_object(user, target),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;

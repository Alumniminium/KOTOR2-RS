//! Combat-relevant item data: baseitems.2da weapon/armour columns plus the
//! UTI PropertiesList, resolved to the few numbers combat uses.
//!
//! Property ids come from itempropdef.2da (K2): 0 Ability, 1 Armor,
//! 5 Enhancement, 8 AttackPenalty, 11 Damage, 15 DamagePenalty, 38
//! AttackBonus, 41 ToHitPenalty, 51 Monster_damage. Bonus amounts are
//! CostValue rows of iprp_bonuscost (value column), damage dice are rows of
//! iprp_damagecost / iprp_monstcost (numdice, die).

use k2_data::{GffExt, Tables};
use k2_res::Resources;
use rakata_core::ResourceType;

/// One raw item property, as stored in the UTI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ItemProperty {
    pub property: u16,
    pub subtype: u16,
    pub cost_table: u8,
    pub cost_value: u16,
    pub param1: u8,
    pub param1_value: u8,
}

/// What a weapon does, from baseitems.2da plus properties.
#[derive(Debug, Clone, PartialEq)]
pub struct WeaponInfo {
    pub base_item: u32,
    /// baseitems weaponwield: 1 stun baton, 2 one-handed blade, 3 two-handed
    /// (staff, double blade), 4 pistol, 5 rifle, 6 heavy; 0 = unarmed.
    pub wield: i32,
    pub ranged: bool,
    pub num_dice: i32,
    pub die: i32,
    /// Threat range size: a crit threatens on a natural 21 - crit_threat or more.
    pub crit_threat: i32,
    pub crit_mult: i32,
    /// Preferred attack distance in meters.
    pub range: f32,
    pub attack_bonus: i32,
    pub damage_bonus: i32,
    /// Extra damage dice from Damage / Monster_damage properties.
    pub extra_dice: Vec<(i32, i32)>,
    /// A lightsaber base item (baseitems rows 8-10; damageflags 4096 is plain
    /// energy damage and blasters carry it too).
    pub lightsaber: bool,
    /// DoorCutting property (itempropdef 64, the plasma torch): ignores the
    /// hardness of doors and placeables, like a lightsaber.
    pub door_cutting: bool,
}

impl WeaponInfo {
    /// Fists: 1d3, melee.
    pub fn unarmed() -> Self {
        Self {
            base_item: u32::MAX,
            wield: 0,
            ranged: false,
            num_dice: 1,
            die: 3,
            crit_threat: 1,
            crit_mult: 2,
            range: 0.0,
            attack_bonus: 0,
            damage_bonus: 0,
            extra_dice: Vec::new(),
            lightsaber: false,
            door_cutting: false,
        }
    }
}

/// Armour numbers, from baseitems.2da plus properties.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct ArmorInfo {
    pub base_ac: i32,
    /// Max dex bonus; None = uncapped.
    pub max_dex: Option<i32>,
    pub ac_bonus: i32,
}

/// Everything combat wants to know about one item template.
#[derive(Debug, Clone, Default)]
pub struct ItemInfo {
    pub base_item: u32,
    pub properties: Vec<ItemProperty>,
}

impl ItemInfo {
    pub fn load(res: &Resources, template: &str) -> Option<Self> {
        let gff = k2_data::read_gff(res, template, ResourceType::Uti).ok()?;
        let root = &gff.root;
        let properties = root
            .list("PropertiesList")
            .iter()
            .map(|p| ItemProperty {
                property: p.uint("PropertyName").unwrap_or(0) as u16,
                subtype: p.uint("Subtype").unwrap_or(0) as u16,
                cost_table: p.uint("CostTable").unwrap_or(0) as u8,
                cost_value: p.uint("CostValue").unwrap_or(0) as u16,
                param1: p.uint("Param1").unwrap_or(0) as u8,
                param1_value: p.uint("Param1Value").unwrap_or(0) as u8,
            })
            .collect();
        Some(Self { base_item: root.uint("BaseItem").unwrap_or(0), properties })
    }

    pub(crate) fn bonus(res: &Resources, tables: &Tables, cost_value: u16) -> i32 {
        tables
            .get(res, "iprp_bonuscost")
            .ok()
            .and_then(|t| t.int(cost_value as usize, "value"))
            .unwrap_or(i32::from(cost_value))
    }

    fn dice(res: &Resources, tables: &Tables, table: &str, cost_value: u16) -> Option<(i32, i32)> {
        let t = tables.get(res, table).ok()?;
        Some((t.int(cost_value as usize, "numdice")?, t.int(cost_value as usize, "die")?))
    }

    /// The item as a weapon (None if its base item has no damage dice and
    /// it is not a creature weapon).
    pub fn weapon(&self, res: &Resources, tables: &Tables) -> Option<WeaponInfo> {
        let t = tables.get(res, "baseitems").ok()?;
        let row = self.base_item as usize;
        let label = t.str(row, "label").unwrap_or("").to_ascii_lowercase();
        let creature_weapon = label.starts_with("creature_item") || label.starts_with("creature_weapon");
        let num_dice = t.int(row, "numdice");
        let die = t.int(row, "dietoroll");
        if num_dice.is_none() && !creature_weapon {
            return None;
        }
        let ranged = t.int(row, "rangedweapon").unwrap_or(0) != 0;
        let mut w = WeaponInfo {
            base_item: self.base_item,
            wield: t.int(row, "weaponwield").unwrap_or(0),
            ranged,
            num_dice: num_dice.unwrap_or(0),
            die: die.unwrap_or(0),
            crit_threat: t.int(row, "critthreat").unwrap_or(1).max(1),
            crit_mult: t.int(row, "crithitmult").unwrap_or(2).max(1),
            range: if ranged { t.float(row, "prefattackdist").unwrap_or(15.0).clamp(4.0, 15.0) } else { 0.0 },
            attack_bonus: 0,
            damage_bonus: 0,
            extra_dice: Vec::new(),
            lightsaber: !ranged && label.contains("lightsaber"),
            door_cutting: false,
        };
        for p in &self.properties {
            match p.property {
                // Enhancement: attack and damage.
                5 => {
                    let v = Self::bonus(res, tables, p.cost_value);
                    w.attack_bonus += v;
                    w.damage_bonus += v;
                }
                38 => w.attack_bonus += Self::bonus(res, tables, p.cost_value),
                8 | 41 => w.attack_bonus -= Self::bonus(res, tables, p.cost_value),
                15 => w.damage_bonus -= Self::bonus(res, tables, p.cost_value),
                11 => {
                    // Damage bonus: cost value is a row of iprp_damagecost,
                    // either a flat amount (label "1".."5") or dice.
                    if let Some(d) = Self::dice(res, tables, "iprp_damagecost", p.cost_value) {
                        w.extra_dice.push(d);
                    } else if let Ok(tc) = tables.get(res, "iprp_damagecost") {
                        w.damage_bonus += tc.str(p.cost_value as usize, "label").and_then(|s| s.parse().ok()).unwrap_or(0);
                    }
                }
                64 => w.door_cutting = true,
                51 => {
                    if let Some(d) = Self::dice(res, tables, "iprp_monstcost", p.cost_value) {
                        w.extra_dice.push(d);
                    }
                }
                _ => {}
            }
        }
        // A creature weapon with no dice of its own deals its monster damage
        // as the base roll, or 1d4.
        if w.num_dice == 0 {
            let (n, d) = w.extra_dice.first().copied().unwrap_or((1, 4));
            if !w.extra_dice.is_empty() {
                w.extra_dice.remove(0);
            }
            w.num_dice = n;
            w.die = d;
        }
        Some(w)
    }

    /// The item as armour (body slot, creature hide, or anything with AC
    /// properties).
    pub fn armor(&self, res: &Resources, tables: &Tables) -> ArmorInfo {
        let mut a = ArmorInfo::default();
        if let Ok(t) = tables.get(res, "baseitems") {
            let row = self.base_item as usize;
            a.base_ac = t.int(row, "baseac").unwrap_or(0);
            a.max_dex = t.int(row, "dexbonus").filter(|&d| d >= 0);
        }
        for p in &self.properties {
            match p.property {
                1 => a.ac_bonus += Self::bonus(res, tables, p.cost_value),
                20 => a.ac_bonus -= Self::bonus(res, tables, p.cost_value),
                _ => {}
            }
        }
        a
    }
}

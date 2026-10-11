# Crafting, workbenches, lab stations, locks

Code: `crates/k2-world/src/craft.rs`. Harness: `k2world craft MODULE`.

## How the game opens the screens

- Workbench placeables (`plc_workb_rep`, `workb_rep001`, Peragus
  `workbench`) are useable, with OnUsed `a_compdlg` (starts the placeable's
  own conversation `workbnch`, or `workbnch_tut` in 001ebo) or `k_workbench`
  (Peragus: plays `000_workbench` while global `000_Workbench` > 0, then
  decrements it; otherwise opens the screen directly).
- Dialog replies run the screen scripts:
  - `k_workbench_bd`: `ShowUpgradeScreen(OBJECT_INVALID, GetFirstPC(), 0, 1, "")`
    (create/break down only)
  - `k_workbench_bdm`: same with override 2DA `itemcreatemira`
  - `k_workbench_t3`: `ShowUpgradeScreen(OBJECT_INVALID, GetFirstPC(), 1, 0, "")`
    (upgrade only)
  - `k_workbench`: `(OBJECT_INVALID, GetLastUsedBy(), 0, 0, "")` (both)
- Lab stations (`plc_workb_med`, `workb_med001`, tag MedBen) use `a_compdlg`
  into `000_medstation`; reply "[Use the Lab Station now.]" runs
  `k_medstation`: `ShowChemicalUpgradeScreen(GetLastUsedBy())`.
- Signature: `ShowUpgradeScreen(oItem, oCharacter, nDisableItemCreation,
  nDisableUpgrade, sOverride2DA)`. nwscript.nss says oCharacter is "NOT
  IMPLEMENTED" for skills; we still use it (falling back to the leader).

World events: `UpgradeScreen { item, character, item_creation, recipes }`
when upgrading is enabled, otherwise `ItemCreateScreen { station,
character, recipes }`. `World::craft.screen` holds the open screen;
`close_craft_screen()` clears it.

## Recipes

- Workbench: `itemcreate.2da` (or the override, e.g. `itemcreatemira`,
  `tutitemcreate`). Columns: label = item template, `skill` = rank needed,
  `group` = screen tab, `level` = min character level, `align`, `base_skill`
  = SKILL_* index, `overridetag` = tag the created item gets (tutorial plot
  checks such as K_REPAIR_PART, K_COMPUTER_SPIKE).
- Lab: `chemicalcreate.2da`: label, `skilllevel`, `group`, `base_skill`
  (7 Treat Injury for medical, 1 Demolitions for traps/grenades).
- SKILL_*: 0 Computer Use, 1 Demolitions, 2 Stealth, 3 Awareness,
  4 Persuade, 5 Repair, 6 Security, 7 Treat Injury.
- COMPONENTS and CHEMICALS are stackable plot items `compont_00001` (tag
  Components, base item 96) and `chem_00001` (tag Chemicals, base item 95).
  `GetItemComponent`/`GetChemicals` (and the *PieceValue variants) count them
  in the party inventory.

Approximations (no table holds these; the engine computes them):
- Checked against the install: itemcreate.2da and chemicalcreate.2da have no
  cost or component-count column (the `dunno` column is 0 in all 209 rows).
  skillvsitemcost.2da (devicecostmax against skill requirements) has no
  stated use as a crafting table. Web sources give no formula: GameBanshee
  says the most expensive items take about 400-500 components and that an
  Advanced Medpac (80 credits) breaks into 4 components, while one forum user
  got chemicals from the same medpacs. None of these are used.
- Cost to create = `max(1, ceil(value / 10))` units, value = UTI Cost (guess).
- Breakdown yield = `floor(cost * min(1, 0.25 + 0.025 * rank))`, at least 1.
  Rank = Treat Injury at a lab, else the item's itemcreate base_skill, else
  Repair (dialog.tlk 76291 says yield depends on value and skill). Plot items
  and raw materials can't be broken down; the lab only takes chemicalcreate
  items and the workbench everything else. Installed upgrades come back.

## Upgrades

- `upgradetypes.2da`: 0 saber crystal, 1 lens, 2 emitter, 3 saber cell,
  4 targeting, 5 power pack, 6 firing chamber, 7 grip, 8 blade/edge,
  9 melee cell, 10 armour overlay, 11 armour underlay.
- `upgrade.2da` maps upgrade templates to those types; `upcrystals.2da` lists
  lightsaber colour crystals with the model per saber type (short, long,
  double), e.g. GREEN `u_l_colo_02` -> `g_w_lghtsbr03`.
- Slots per item kind (from baseitems itemclass / rangedweapon / armortype):
  lightsaber = colour, power crystal, lens, emitter, cell; ranged =
  targeting, firing chamber, power pack; melee = grip, edge, cell; body
  armour = overlay, underlay.
- Fit rule, inferred from the shipped UTIs: an upgrade fits when its type
  matches the slot and its UpgradeLevel <= the item's UpgradeLevel. The data
  is consistent with that: targeting upgrades are all level 1, firing
  chambers 2, power packs 3, saber cells and crystals 0, lenses/emitters 1;
  armour overlays/underlays span 0-4 matching armour levels 1-4; weapons with
  UpgradeLevel 0 take nothing.
- Installed upgrades live in `World::craft.installed` (item -> slot ->
  template); their PropertiesList is merged into the item's properties in
  `item_info`, so combat sees them (verified: Accuracy Scope Mark I, property
  38 +1, raises attack 3 -> 4). Upgrading a stacked item splits one unit off
  first; upgraded items never merge into stacks. A colour crystal sets
  `item_model_override(item)` for the held-weapon model (kotor2 side still to
  consume it).

## Locks and Security

- Lock DC is the blueprint OpenLockDC (UTD/UTP), loaded on demand.
  `lock_info(user, target)` gives DC, key requirement, key possession, the
  user's Security rank and whether a take-20 succeeds.
- `try_unlock(user, target)`: key -> success; KeyRequired without key ->
  error; otherwise Security rank + 20 out of combat (take 20) or + d20 in
  combat vs DC. Success unlocks, fires OnUnlock and uses the object (door
  opens, container opens); failure fires OnFailToOpen. Emits `SkillCheck`.
  `ActionUseSkill(SKILL_SECURITY, target)` from scripts goes the same way.
- Traps on doors, placeables and mines: see traps-stealth-influence.md
  (GetIsTrapped is answered by traps.rs; craft.rs no longer shadows it).
  Bash on placeables: bash.rs.

## Skills

`skill_rank(id, skill)`: explicit ranks (`set_skill_rank`), else the UTC
SkillList of the creature's template, else 4 for the PC (no character
generation yet). `GetSkillRank`/`GetSkillRankBase`/`GetHasSkill` use it;
GetSkillRank returns -1 for an invalid skill or non-creature, as the engine
does. Computer panels are dialogs whose conditionals call GetSkillRank and
count spikes via GetItemPossessedBy/stack size, which now work.

## Verified (k2world craft)

- 232tel: workbench dialog -> ItemCreateScreen and UpgradeScreen; 209
  itemcreate recipes; created Pinpoint Scope Mark I (60 -> 55 components);
  broke down a blaster (+2 components); scope upgrade +1 attack and removal;
  colour crystal model swap; lab station dialog -> ItemCreateScreen
  (chemicalcreate, 69 recipes); created a Medpac (30 -> 26 chemicals); broke
  one down; Security Door DC 28 fails at rank 4 (24) and opens at rank 28.
- 103per (k_workbench path), 001ebo (tutorial bench, `tutitemcreate`, T3-M4
  picks DC 21 with its own Security 6).
- `k2world all --seconds 60`: 82 modules, 0 panics, 0 VM errors, 0
  unimplemented routines.

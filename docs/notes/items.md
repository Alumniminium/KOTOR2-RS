# Items, inventory, containers, stores (k2-world inventory.rs, store.rs)

## Item templates (UTI, K2)

Fields used: TemplateResRef, Tag, LocalizedName, Description,
DescIdentified, BaseItem, ModelVariation, TextureVar, BodyVariation,
StackSize, Charges, Cost, AddCost, Plot, Stolen, Identified, UpgradeLevel
(K2 only: the upgrade tier), PropertiesList. Shipped UTIs carry
AddCost == Cost, so the item value is Cost alone.

K2 item names carry developer tags in braces ("{01}Blaster Pistol",
"{hvy 01}Battle Armor"); `strip_notes` removes them for display.

`World::item_def(template)` caches an `ItemDef` per template, which joins
the UTI with baseitems.2da (label, itemclass, stacking, equipableslots,
droidorhuman, defaulticon, defaultmodel).

- Icon: `i` + itemclass + `_` + %03d ModelVariation (iw_blstrpstl_001,
  ia_class4_001, ii_medeqpmnt_002), else baseitems defaulticon. Survey
  (`k2world items`): 1114 templates, all named, 30 without an icon, all of
  them creature hides and claws (never shown).
- An item's UTI can live in a module other than the loaded one (the save
  stores only the template name, and items keep the template of the area they
  were found in: chem_002 and plasmatorch in the 101PER family, firstnamesstarpo
  in 501OND). `load_item_def` falls back to every module's capsules when the
  loaded resources lack it.
- Quick bar item powers (medpacs, stims, grenades, shields: spells.2da rows of
  usertype 4) have an empty iconresref, so their bar entry takes the icon of
  the item that casts them (`World::usable_talents`, `item_for_spell`). The
  textures are in swpc_tex_gui.erf (ii_MedEqpmnt_*, ii_Adrnaline_*,
  ii_CmbtShot_*); the doubled "ii" is the real name.
- Model: itemclass + `_` + %03d ModelVariation (w_blstrpstl_001), else
  defaultmodel (I_Null = none). All 243 weapon templates resolve.
- equipableslots is a bitmask over INVENTORY_SLOT_*: 0 head, 1 body,
  3 hands, 4 right weapon, 5 left weapon, 7/8 arms, 9 implant, 10 belt,
  14-16 creature weapons, 17 creature hide. 0x10 alone = two-handed.
- droidorhuman: 0 anyone, 1 organics only, 2 droids only (racial type 5).
- stacking: largest stack (99 for most, 15000 credits/components).

## Party inventory

K2 shares one inventory across the party. It is the inventory list of the
PC object (`World::player`), also while SwitchPlayerCharacter has the PC
stashed (the prologue's T3-M4 section); enter_module keeps those items.
Anything given to a party member lands there (`store_item` redirects), and
units of the same template merge into one stack up to `stacking`.
Equipment stays per creature. Credits live on the PC (`credits`,
`add_credits`).

Script semantics implemented that way: CreateItemOnObject (with K2's
nHideMessage), GetItemPossessedBy (party inventory plus the creature's own
equipment), GetFirst/NextItemInInventory, GiveItem / ActionGiveItem /
ActionTakeItem, Get/Give/TakeGold, GetInventoryDisturbItem, OpenStore,
ActionEquipItem / ActionUnequipItem, ActionEquipMostDamagingMelee/Ranged
(best average damage among usable weapons), DestroyObject on items. Usage
ranking across all shipped NCS: CreateItemOnObject (245 scripts),
GetItemPossessedBy (137), GetItemInSlot (94), GetFirst/NextItemInInventory
(65), GetBaseItemType (62), ActionEquipMostDamaging* (61), stack size
routines (30), OpenStore (19), gold (16).

## Equipment

`World::equip(creature, item, slot)` checks the slot mask, droid/organic
restriction and the two-handed rule, takes one unit off a stack, moves the
previous item back to the inventory and emits `EquipmentChanged`. Armour
(slot 1) recomputes the body: baseitems bodyvar letter picks the
appearance.2da model column (modela..modelm) and TextureVar the texture, so
a Miner Uniform turns pmbam into pmbcm; that emits `AppearanceChanged`.
Combat stats read equipment live, so nothing else is needed.
Feat/proficiency requirements are not enforced.

Body resolution (`Tables::creature_models`, same rule as reone): armour
letter L (baseitems bodyvar) picks appearance.2da `modelL` and `texL`; the
texture is `texL` + %02d TextureVar, else `texL` + "01", else the bare name.
A letter the row has no model for falls back to letter a for model and
texture together. A letter whose `texL` is EMPTY keeps the model's own
textures: party bodies such as P_AttonBB, P_MiraBB, P_VisasBB and N_OndSoldM1
carry them inside the MDL (221 rows have a model without a tex column).
Borrowing `texa` there (the old code) painted the plain body skin
(pmbamc01) over Atton's, Mira's and Visas's basic clothes; fixed, see
rendering.md. UTC BodyVariation/TextureVar are 1/1 or 0/0 in every shipped
creature, so they are not used. Dark side `texLevil` columns are unused.

Head gear (slot 0, baseitems Mask, itemclass I_Mask): the item model
i_mask_NNN (NNN = UTI ModelVariation, 3..14 in use: goggles, ear
amplifiers, breath masks, Sith masks) hangs off the head model's `MaskHook`
node (`kotor2/src/armor.rs`, rebuilt on EquipmentChanged for slot 0 and
after every respawn). The head itself stays. Droid sensor items have no
model (I_Null); heads without a MaskHook (creature models) show nothing.
Gloves, belts, implants and arm bands have no model in K2.

Saves store the derived body model/texture, so `World::load` and
`restore_module` re-derive every creature's body from appearance and worn
armour (`refresh_all_bodies`); a save made before the texL fix showed Atton
with the wrong skin until then.

Equipment screen: world events are not drained while a menu is open, so
armour swapped there changes the body when the menu closes (all queued
AppearanceChanged events collapse into the final state).

Tools: `cargo run --release -p k2-data --example armorsurvey [ROW...]`
(per appearance row: model/tex per letter, missing texture variations) and
`--example utcsurvey` (every GIT creature: appearance, UTC variation
fields, worn armour, resolved body/texture/head). Test hooks:
`K2_CAM_VIEW=DIST,YAW_DEG,PITCH_DEG` starts the follow camera at that
distance/pitch, YAW_DEG around from behind the leader (180 = front);
`--give a_robe_02,a_helmet_05` equips; `K2_GUI_OPEN=equip` with
`K2_GUI_CLICKS=equip:BTN_INV_BODY,equip:LB_ITEMS#0,...` drives the screen.

Bevy (`kotor2/src/weapons.rs`): right/left weapon models hang off the body's
`rhand`/`lhand` nodes and follow the animation; rebuilt on
EquipmentChanged. AppearanceChanged despawns the object root so it is
respawned with the new body. Test hook: `kotor2 --give w_blaste_01,a_heavy_01`.

## Weapon sets (X key)

Creatures have two weapon loadouts (right + left hand each), like the
original equipment screen's Config 1 / Config 2. The active pair always
lives in slots 4/5, so combat stats, held models, sounds, dual-wield and
two-handed rules, GetItemInSlot and ActionEquipMostDamaging* need no
changes. The inactive pair sits in internal slots 18/19
(`SLOT_RIGHTWEAPON2` / `SLOT_LEFTWEAPON2`, not INVENTORY_SLOT_* values) of
the same `equipped` map, so item enumeration, module carry-over and
destroy/unequip handle it already. `CreatureState::weapon_set` (0 or 1)
says which set is active.

- `World::swap_weapon_sets(creature)` trades 4<->18 and 5<->19, flips
  `weapon_set`, emits EquipmentChanged for 4 and 5 (held models rebuild)
  and `WeaponSetSwapped { creature, set }`. False when no weapon is
  equipped in either set.
- `weapon_set_slots(creature, set)` gives the (right, left) slots where
  set 0 or 1 lives right now; `can_equip` / `equip` accept 18/19 with the
  same slot-mask, droid and two-handed rules, applied per set (a
  two-hander in 18 frees only 19).
- Saves: `weapon_set` is a new field with a default (0), older saves
  load unchanged; the digest includes it.
- kotor2: X (`weapons.rs`, not in dialog, menus or pause) swaps the party
  leader's sets with a floating "Weapon Set N" line and a UI click, or an
  error line and sound when there is nothing to swap to. The equipment
  screen shows both pairs (the buttons use stable logical slots 4/5 for
  set 1 and 18/19 for set 2; `physical`/`logical` translate to the world's
  current layout) and BTN_SWAPWEAPONS swaps the shown member.
- Draw animation: any change of the live hands (equip, unequip, swap)
  sets `CreatureState::equip_until` (`World::weapon_busy`, `EQUIP_SECONDS`
  = 1.0); `step_attack` holds the round while it is positive. The
  presentation (`weapons.rs`) plays the creature's `equip` clip: it is in
  S_Male02 (1.0 s, event `draw_weapon` at 0.567 s) and reaches every
  humanoid through the supermodel chain; S_Female02 only has `itemequip`
  (use item on other), so it is not used. There are no per-weapon-class
  draw clips (g1r1 etc. are ready stances, not draws). Old models vanish
  when the clip starts, new ones attach when the clip time reaches
  `draw_weapon` (or at once when something else, a cutscene clip or
  walking, replaced the clip). Sounds: baseitems powerdownsnd when a
  lightsaber or stun baton is put away, powerupsnd when drawn
  (cb_ls_powerdown / cb_ls_powerup; poweredsnd is the hum, not played yet).
  Creatures lacking the clip swap models immediately (no `equip` in the
  supermodel chain: n_darthmalak, n_darthnihilus, n_gammorean, n_selkath,
  n_jedirobef, h_m_hi01, h_m_hibody, pmbh02, p_hk47; scan with a scratch
  k2-mdl walk over chitin MDLs). A change made while paused (a menu open)
  also swaps at once, since no clip runs then. The draw sounds (powerdownsnd
  then powerupsnd) play in every case, so scripted ActionEquipMostDamaging*
  and menu equips sound the same. Verified from the
  log of `K2_SWAP_WEAPONS=60 kotor2 --module 102per --give ...`: "equip
  clip starts at 1.15 s (length 1.00)", "draws at 1.74 s (clip time 0.58)".
- Test hooks: `--give TEMPLATE@2` equips into the second set;
  `K2_SWAP_WEAPONS=N` presses X once at frame N. Headless tests:
  `crates/k2-world/tests/weapon_sets.rs`.

## Off-hand rules

The left hand never holds a weapon alone. Per weapon set (slots 4/5 and
18/19):

- `can_equip` for a left slot needs a one-handed weapon in the right slot
  of the same set (`NeedsMainHand`, `TwoHanded`) and a partner of the same
  kind (`NoDualWield`): `ItemDef::dual_class` from baseitems weaponwield,
  1 = one-handed melee (stun baton, blades, lightsabers, short lightsabers
  mix freely), 2 = pistols. Two-handed weapons, rifles and creature weapons
  are class 0. The class split is our reading of weaponwield (the shipped
  data has no explicit pairing table); feats only give penalties, not a
  gate. The main-hand item itself cannot be moved to the left slot.
- `equip_candidates` for a left slot is therefore empty while the right
  hand is empty.
- Equipping a new main-hand weapon that cannot pair with the held off-hand
  weapon (two-handed or another kind) sends the off-hand weapon back to
  the inventory.
- `unequip` of the main hand (also script ActionUnequipItem and anything
  through `detach_item`, e.g. DestroyObject) moves the off-hand weapon into
  the main hand: `World::repair_weapon_slots`.
- `repair_weapon_slots` also runs on every creature after `World::load`,
  `restore_module` and blueprint spawn, so old saves with an off-hand-only
  setup (or an off-hand weapon beside a two-hander) load repaired.
- X / BTN_SWAPWEAPONS trades whole valid pairs, so it cannot create an
  off-hand-only state.
- Headless tests: `crates/k2-world/tests/offhand.rs`.

## Containers

Placeables with HasInventory, and dead non-party creatures with items, open
a loot window instead of firing OnUsed: `open_container` (locked ->
OnFailToOpen, else open animation + OnOpen + `ContainerOpened`),
`take_item` / `take_all` (OnInvDisturbed, `ItemReceived`),
`close_container` (OnClosed + close animation + `ContainerClosed`). K2 UTCs
have no Lootable field; enemy loot comes from death scripts or BodyBag
placeables (bodybag.2da), which is not modelled yet.

## Stores (UTM)

GIT StoreList entries have no Tag; the object takes the UTM Tag so
GetObjectByTag finds it. The UTM (MarkUp, MarkDown, BuySellFlag bit 0 buy /
bit 1 sell, OnOpenStore, ItemList with Infinite) is read on first open and
its stock becomes item objects owned by the store. Several stores (the 202TEL
black market) ship empty and are stocked by scripts. Buy price = value *
(MarkUp + bonus) / 100, sell price = value * (MarkDown + bonus) / 100, both
at least 1 credit; plot items cannot be sold; infinite stock is copied.

## Harness

    k2world inv MODULE [--seconds N]   party inventory/equipment, loot the
                                       nearest container, equip what fits
    k2world store MODULE               list stores, buy the cheapest item,
                                       sell it back
    k2world items                      resolve names/icons/models of all UTIs

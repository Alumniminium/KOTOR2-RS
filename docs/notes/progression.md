# XP, level up, Force powers and talents

k2-world: progression.rs (XP, levels, known feats/powers) and powers.rs
(talents, casting, effects, saves). kotor2: progress_ui.rs (level up
screens, HUD markers, action bar, cast visuals).

## XP

- GiveXPToCreature / SetXP / GetXP. XP given to the PC (or any party
  member) is shared: every party member and every spawned available NPC
  gets the same amount, so companions keep pace as in K2.
- GivePlotXP(plot, pct): plot.2da `xp` x pct / 100 (rows are labelled
  telos_plot_base_xp etc).
- Journal XP: AddJournalQuestEntry pays plot.2da[category PlotIndex].xp x
  entry XP_Percentage / 100 from global.jrl, once per (quest, entry).
- Kill XP: a party member killing a hostile pays xptable.2da[party leader
  level - 1][victim ChallengeRating] (UTC float, rounded). The table holds
  100..400 for CR 0..8 at level 1, 3000 for CR 9 and up at level 1, and at
  level 50 25 for CR 0-42, then 75, 75, 100, 150, 200, 225, 300, 375 for CR
  43-50. xpbaseconst.2da (leveldiff -8..8 with balance
  4.00..0.00 and a bonus column) is not applied: no source gives the way the
  engine combines it with xptable, and the StrategyWiki K1 example (level 2,
  CR 5 = 150 XP) does not match this table's K2 row (250), so the rule is
  unverified.
- Thresholds: exptable.2da (level 2 = 1000, 3 = 3000, 5 = 10000, ...).
  `World::level_up_available(id)`; WorldEvent::LevelUpAvailable fires once
  per threshold for the PC. Other creatures level automatically
  (`progression.auto_level_pc` makes the PC do so too, for tests).

## Level up rules as implemented

- Class = the creature's last class (AddMultiClass appends one at level 0,
  so the next level goes there).
- HP: hitdie + Con mod (max roll, as KotOR). FP: forcedie + Wis mod for
  Force classes. Creatures whose UTC has no Force points get the class pool
  (sum forcedie x level + Wis mod x Force levels) the first time their
  powers are looked at (Kreia's UTC carries 0).
- Skill points: skillpointbase + Int mod, min 1. Class skills
  (skills.2da `<cls>_class`) cost 1 and cap at level + 3, others cost 2 and
  cap at (level + 3) / 2. Ranks above a cap from before are kept.
- Attribute point at total levels 4, 8, 12, ...
- Feats: featgain.2da `<cls>_reg + <cls>_bon` picks; selectable when
  feat.2da `<cls>_list` is 1 or 2, mincharlevel, prereqfeat1/2 and
  orreqfeat0-4 hold; granted ones at `<cls>_granted` (and for the PC
  `<cls>_pc_granted`) arrive automatically. `<cls>` is classes.2da
  `featgain` lowercased (jgd, jcn, jsn, sol, ...).
- Powers: classpowergain.2da column = classes.2da spellgaintable; available
  when spells.2da usertype = 1, the class column (guardian, consular,
  sentinel, weapmstr, jedimaster, watchman, marauder, sithlord, assassin)
  is <= the class level, and `prerequisites` (underscore list) are known.
- Auto picks: primaryabil, feats by `<cls>_recom`, powers by light_recom,
  skills by `<cls>_reco` on class skills.
- Fresh PC (no template, Jedi Guardian 1): level 1 granted feats plus the
  level 1 power picks by recommendation. Note: K2's chargen classes are the
  three Jedi classes, not K1's Soldier/Scout/Scoundrel.

## Talents and casting

- Talent categories (spells.2da / feat.2da `category`) are matched nibble
  by nibble against the AI's masks, 0xF = wildcard: 0xf8ff any buff
  (0x1808), 0xf1ff any damage power (0x1101, 0x2101, 0x4101, ...), 0xf2ff
  debilitating, 0x1104 melee feats, 0x1111 ranged feats, 0x1408 heal items,
  0x8000 droid items. Inferred from k_inc_gensupport and the tables.
- Candidates: known powers the creature can pay for, CastSpell items it can
  reach (party inventory for party members; itempropdef row 10, Subtype =
  spells.2da row), and known combat feats. Best = highest `pips`/maxcr.
- Cast: close to range (P self, T 2.5 m, S 8, M 15, L 25, W 15), pay
  forcepoints, SpellCastStarted (castanim/conjanim names), impactscript at
  conjtime + casttime/2 with the spell routines answering, SpellImpact.
  In combat the cast lasts a full 3 s round and ends with OnEndCombatRound,
  so k_ai_master keeps driving. Items lose a charge.
- Combat feats arm the next round: Power Attack/Blast -3 attack +5/+8/+10
  damage, Critical Strike/Sniper Shot crit range +2/+4/+6 at -5 defense,
  Flurry/Rapid Shot +1 attack at -4/-2, Multi Shot +2. Values are from the
  feat descriptions, approximate.
- EventSpellCastAt + SignalEvent fire the target's ScriptSpellAt with
  GetLastSpellCaster/GetLastSpell/GetLastSpellHarmful answering.

## Effects (gameplay subset)

Attack/AC/damage/ability/save modifiers, EffectModifyAttacks, movement
speed (scales scripted movement), temporary hit points and damage
resistance (absorbed before HP), Force point damage and Force drain, force
resistance (ResistForce), and debilitation (stun, paralyze, droid stun,
horror, choke, whirlwind, push, sleep, entangle, confusion, fear, mind
trick, crush) which stops the action queue. Timed effects expire. Effects
created while a spell's impact script runs are tagged with the spell
(GetHasSpellEffect, GetEffectSpellId). Regeneration of HP and Force
points (rates from regeneration.2da, items, feats) is in combat.md,
"Regeneration".

Saves: d20 + cls_st_* table + ability (Con/Dex/Wis) + effects vs DC.
GetSpellSaveDC = 5 + caster level + max(Wis, Cha) mod (approximation).
spells.2da has no save or DC column (the formula is likely engine-side).
Web sources for K1 (StrategyWiki droid powers, a stats calculator) give
5 + level + Wis + Cha, the sum, and a forum post adds Cha only; they
disagree with each other and with max(), so the current formula is kept
unverified.

## Harness

    k2world levelup [MODULE] [--class N] [--level N]   PC + Kreia to level N
    k2world powers MODULE [--seconds N]                 party (Kreia) fight, casts logged
    k2world forceusers                                  hostile Force users per module

Results (Aspyr 1.0.2): levelup 101per: the Exile (Jedi Guardian) 1 -> 5
with 4 level ups (Force Sensitive and Unarmed Specialist granted, Toughness,
Two-Weapon Fighting, Improved Power Attack, Force Scream, Resist Energy,
Force Push, Stun picked); Kreia auto-levels 3 -> 5. powers 852nih: the
Dark Jedi cast Drain Life 4 times and use Flurry, Critical Strike and Power
Attack through k_ai_master; kills pay 400 XP; 0 VM errors. `k2world all
--seconds 60`: 82 modules, 0 panics, 0 VM errors.

## kotor2

- HUD `LBL_LEVELUPn` over party members with a level up; the character
  screen's BTN_LEVELUP opens leveluppnl_p (steps: abchrgen_p, skchrgen_p,
  ftchrgen_p, pwrlvlup_p; Recommended fills the auto picks). Finish applies
  it and opens the next one if more XP is banked.
- Action bar BTN_ACTION0..3 (keys 1-4, arrows cycle): attack/feats on the
  target, hostile powers, self powers, items.
- Cast clips: self -> castout1, dark -> castout2, up -> castout3, throw ->
  throwsab; the impact is a short point light flash on the target.
- Test flags: `--levelup-test LEVEL[,STEP]`, `--autocast SPELL`.

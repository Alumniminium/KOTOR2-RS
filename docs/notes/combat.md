# Combat (crates/k2-world: combat.rs, stats.rs, items.rs, factions.rs)

Headless, real-time with 3 s rounds, driven by the game's own AI scripts.

## Using it

    w.player_attack(target);          // leader attacks; replaces its queue
    w.tick(dt);
    for e in w.drain_events() {
        // AttackStarted { attacker, target, anim, fallbacks, ranged }
        // AttackHit { attacker, target, damage, critical }
        // AttackMissed { attacker, target }
        // AttackDeflected { attacker, target }   (after AttackMissed: a bolt turned aside)
        // Projectile { shooter, model, hook, bolt, muzzle, ammo, to, launch, flight, arc, end, blast_model, blast_sound }
        //                                        (projectiles.rs: bolts and grenades in flight)
        // Damaged { object, amount, by }   (any source, incl. EffectDamage)
        // Died { object, killer }          (plus PlayAnimation dead loop)
    }
    w.combat_stats(id)                // level, BAB, Defense, weapons, armour
    w.is_enemy(a, b) / w.reputation(a, b) / w.in_combat(id)

Harness: `k2world fight MODULE [--seconds N] [--settle N]` puts the leader
4 m from the nearest hostile (forcing the nearest creature to Hostile_1 if
there is none), lets the AI react and keeps the player attacking until one
dies, then prints the round log.

## Rules as implemented

- Round = 3 s. Attacks per round: 1, +1 with an off-hand weapon, +1 at BAB
  11+. They resolve at fixed points in the round (0.6 s; 0.6/1.8 s;
  0.5/1.4/2.3 s). At round end the attacker's ScriptEndRound fires.
- Attack: d20 + BAB + str mod (melee) or dex mod (ranged) + weapon attack
  bonus (Enhancement, AttackBonus, minus penalties), with -4/-8 for two
  weapons. A natural 1 misses and a natural 20 hits. A roll of
  21 - critthreat or more is a threat; it becomes a critical when a second
  roll also hits, and damage is multiplied by crithitmult.
- Damage: weapon dice (baseitems numdice/dietoroll) plus extra dice from
  Damage (iprp_damagecost) and Monster_damage (iprp_monstcost) properties,
  plus str mod (melee only) plus enhancement. Minimum 1.
- BAB = sum over classes of cls_atk_N.2da[level-1].bab, where N comes from
  classes.2da attackbonustable. A creature without classes (the
  placeholder PC) is level 1 with BAB 1.
- Defense = 10 + dex mod (capped by the armour's baseitems dexbonus) + UTC
  NaturalAC + armour baseac + Armor AC properties from equipped items + class
  Defense + droid Combat Logic (see "Defense bonuses" below). Other feats
  and force powers reach Defense only through the effects in powers.rs.
- Weapon pick: RIGHTWEAPON, else creature weapons (CWEAPON_R/B/L), else
  unarmed 1d3. Armour: BODY plus CARMOUR (creature hide).
- Reach: melee 1.0 m + both radii; ranged uses baseitems prefattackdist,
  clamped to 4..15 m. Out of reach, the attacker runs in along a walkmesh
  route (walkmesh.md, nav.rs) with sliding. Blocked for 1 s while nearly in reach
  (reach + 1.5 m, height difference < 2 m) means it attacks anyway. Blocked
  for 4 s means it gives up and fires ScriptOnBlocked.
- Damage from any source updates GetLastDamager/GetLastHostileActor and
  fires ScriptDamaged. At 0 HP, min1hp and plot creatures stay at 1; a
  party member is knocked out instead (see "Knocked out and getting up");
  otherwise the creature dies: its queue clears, ScriptDeath fires, the
  dead loop plays (ANIMATION_LOOPING_DEAD = 26), and the corpse is
  destroyed after 12 s unless it is Lootable, DisableFade, plot, a party
  member, or SetIsDestroyable(FALSE) was called. EffectDeath kills outright
  (`harm(.., lethal = true)`), a party member included; a standing min1hp or
  plot creature stays at 1 HP, a knocked-out one stays down (see below).
  A destroyed corpse is invisible to get and iter, so GetObjectByTag stops
  finding it at the destroy (12 s after death). GetReputation still reads the
  dead creature as hostile, because reputation() ignores death (world.md, 101per
  prison door).
- Each attack fires ScriptAttacked (OnPhysicalAttacked) on the target, hit
  or miss.

## Defense bonuses (stats.rs, combat.rs)

- Class Defense: classes.2da armorclasscolumn names a column of
  acbonus.2da (scd, sol, sct, jdc, jds, jdg, sas, sld, sma, jwa, jma, jwm,
  tec), read at row level - 1 and summed over the creature's classes like
  BAB, except the droid classes (below). Soldier 0 at levels 1-5, 2 at 6-11,
  4 at 12-17, 6 at 18; Jedi Guardian 2 at level 1. The row convention is
  checked against the droid Combat Logic text (feat 110: +2 at level 1 and +2
  every 6 levels), which matches the Scoundrel column row for row.
- Droid Defense (CombatDroid 6 and ExpertDroid 7 use the SCD column, the same
  ladder as feat 110): the class term and the Combat Logic feats are one
  bonus, so the larger applies, never the sum. Feat 110 is 2 + 2 * (level / 6)
  (TLK 38673). Tactician (111) is +4 and Battle Droid (112) +6, flat, each
  replacing the feat below (TLK 38674, 38675), so the highest owned one sets
  the value. A non-droid's feat 110 is added to its class terms. Pending an
  in-game sheet reading of HK-47 and T3-M4 Defense: p_hk47 (CombatDroid 6,
  owns 110 and 111) gets +4 (Defense 19; a sum model would give 23), p_t3m4
  (ExpertDroid 3, owns 110) +2 (Defense 14; sum model 16).
- Deflection: a blaster bolt (ranged attack that hits) is turned aside when
  the target's d20 + deflection bonus is greater than the attack total
  (roll + attack bonus). Jedi chain 55 Jedi Defense, 1 Advanced, 24 Master
  (TLK 1179, 1126, 1148): +0 (guess, TLK 1179 gives no number), +3, +6,
  each replacing the one before. Any feat of the chain opens the gate,
  which needs a lightsaber in hand, and the highest applies.
  Deflect (168, TLK 48150) is 1 + (level - 1) / 2. The better of the two
  applies (stacking is a guess: the feat text does not say). Winning by 10
  sends the bolt back at the attacker; not modelled. Melee is never deflected.
- Not modelled: flat-footed loss of dodge bonuses, Uncanny Dodge (keeps the
  Dex bonus against surprise, feat 56/57), the Jedi Defense text's
  "Certain items may apply a bonus", and the bolt-back.

Duel check (`cargo run --release -q -p k2-world --example duel -- 101per CLASS
50 2`, unarmed Exile, two mining droids, same seeds): before, Soldier 100%
wins, 1.4 HP lost, leader Defense 12; Jedi Guardian 100%, 1.8 HP lost,
Defense 12 (both before class Defense was counted). After, with class and
droid Defense: Soldier 100% wins, 1.5 HP lost (worst 4 HP left), leader
Defense 12, droids 7 and 8, 18 of 356 swings hit; Jedi Guardian 100%, 1.8 HP
lost (worst 3 left), leader Defense 14 (the +2 SCD term), 22 of 459 swings hit.

## Factions

repute.2da is square: row = the faction holding the opinion, column = the
faction it is about. Column names come from the `label` column (Hostile_1 =
faction 1). The Player faction (row 0) has no column, so opinions about
faction 0 are read from row 0. 0..10 is hostile, 90..100 friendly, anything
else neutral. Party members always count as Friendly_1 (2).
AdjustReputation overrides one direction (source faction -> target faction).

## How the K2 AI drives it

The default creature scripts are thin wrappers: k_def_percept01,
k_def_combend01, k_def_attacked01, k_def_damage01 and so on call
`ExecuteScript("k_ai_master", OBJECT_SELF, 100x)`, and k_ai_master switches
on GetRunScriptVar():

    1001 heartbeat   1002 perception   1003 end of combat round
    1004 dialogue    1005 attacked     1006 damaged    1007 death
    1008 disturbed   1009 blocked      1010 force affected
    1011 global dialogue end           1012 path blocked
    3001..3003 "determine combat round" shortcuts

The same numbers are signalled as EventUserDefined when the spawn-in
condition flags ask for it (SW_FLAG_EVENT_ON_*). Perception (1002) and
round end (1003) both call GN_DetermineCombatRound (k_inc_generic). It
picks a target with GetNearestCreature(CREATURE_TYPE_REPUTATION,
REPUTATION_TYPE_ENEMY, ...), tries talents (GetCreatureTalentBest returns
an invalid talent here, so it falls through), and ends in ActionAttack.
Nothing is hard-coded on our side: hostiles engage through those scripts.
The game ships the .nss sources of these scripts in scripts.bif, so they
can be read directly.

Perception for combat: besides the world's party perception, every creature
with an OnNotice script notices non-party creatures hostile to it within
its perception range (party members spotting enemies, predator/prey).
GetNearestCreature supports criteria 0 racial type, 1 player char, 2 class,
3 reputation (corpses skipped unless IS_ALIVE is also given), 4 is alive,
7 perception (range-based; seen/heard variants).
Our nearest_creatures (combat.rs) also skips knocked-out creatures (down, not
dead) in the reputation search. That is our assumption, not sourced: no engine
or script evidence in the install says whether a knocked-out creature counts as
a corpse for it. The saber friend search (projectiles.rs) uses the same code, so
a knocked-out friend is not picked; the test a_friend_knocked_down_before_the_throw_is_not_reached
pins our rule, not the game's.

Also implemented: GetLastAttacker, GetLastDamager, GetLastHostileActor,
GetLastHostileTarget, GetLastKiller, GetAttackTarget,
GetAttemptedAttackTarget, GetIsInCombat (round running or hit/attacked in
the last 6 s), GetLastAttackResult, GetLastAttackAction,
GetTotalDamageDealt/GetDamageDealtByType (last damage taken),
GetCombatActionsPending, CancelCombat, SetIsDestroyable, Set/GetNPCAIStyle,
Set/GetPartyAIStyle, GetGoingToBeAttackedBy, GetWeaponRanged,
GetIsWeaponEffective, GetAC, the GetFaction*Member family,
GetFactionAverageLevel and GetFirst/NextFactionMember.

## Animation names (K2 supermodels)

Combat clips in s_male02/s_female02 follow `<g|b><wield>a<n>` for attacks
(g = melee/generic, b = blaster style), `<g><wield>d1` dodge, `g<wield>r1`
ready, `g<wield>g1` got hit, `g<wield>w1` walk. Here `wield` is the
baseitems weaponwield value (1 stun baton, 2 one-hand blade, 3 two-hand,
4 pistol, 5 rifle, 6 heavy) and 8 is unarmed. Droid supermodels only carry
`b0a1..b0a4`. AttackStarted gives the best guess plus fallbacks; the Bevy
side plays the first clip the model has.

Measured on all 3237 MDLs in the install: no model has a `b4` clip (pistols),
b5, b6, b7 and b9 attacks carry no events, and the `g4`, `g1`, `g2`, `c2`,
`c3`, `c4` and `g8` attacks are melee swings (swingshort/swinglong/hit/clash
events). So a pistol shot cannot use `g4`: that is the melee swing the
player saw. The attack list is `combat::attack_clip_list`, the same list the
client walks (first clip the model has). The wield class is not the animation
family (animations.2da descriptions): `combat::blaster_family` maps pistol
(wield 4) to family 5 (single blaster), rifle (5) to 7, heavy (6) to 9, and
two pistols (dual blasters) to 6; melee wields 1-3 are their own family. So a
ranged attack is b<family>, then the b5, b7 and b9 fallbacks and the droid b0a
clips, with no melee, unarmed or throw family at all; a blaster with no clip
idles instead of swinging. The family mapping is from the descriptions, not
yet checked in game.
The in-combat stance is `combat::ready_clips(family)`: g<family>r1 first
(`World::main_hand_family`, two pistols giving family 6), then creadyr and
creadyrtw, the creature ready clips that creature and droid bodies carry (67
and 34 models). Unarmed tries g10r1 before g8r1; that order is an inference
from the install, where both exist on human chains. Out of combat a creature
stands at pause1.

Throw clips: the client plays `throwgren` / `throwsab` only for a
SpellCastStarted whose castanim is "throw" (progress_ui.rs cast_clips). Those
come from grenade and saber-throw talents. Measured: `m2`/`m3`/`m4`/`m10`
attacks are melee swings too (swing events), so they never belong to a
ranged list.

GetAttackTarget answers with the target of a running Attack action from its
first frame (the companion scripts k_hen_percept01 / k_ai_master 2002 check the
leader's attack target before they join a fight). A queue holding only a
Follow still counts as idle for the renotice of hostiles (`renotice_due`); the
QUEUEEMPTY gates cannot be reached by a follow-only companion (no attack target
exists), so GetCurrentAction keeps reporting ACTION_FOLLOWLEADER.
Off hand: the dual-blaster family (6) needs a pistol in slot 5. The creature
weapon slots (14-16) are natural weapons, so they never count as an off hand,
and main_hand_wield searches them only for the main hand.
Party members notice what is hostile to the party and whatever attacks a
party member (`perceive_hostiles`); whether a neutral attacker then counts as
an enemy is still the faction table's answer (GetIsEnemy), so it does not
engage. ActionSwitchWeapons swaps only to a set that holds a weapon.

Shouts: `World::hear_speech` (SpeakString, ActionSpeakString) gives every
listening creature within the talk volume's range whose listen pattern matches
the text (ignoring case) an OnDialogue (ScriptDialogue) event with
GetListenPatternNumber = the pattern's number and GetLastSpeaker = the speaker.
The shipped scripts then call GN_RespondToShout (k_hen_*dlg via 2004,
k_def_dialogue01 via 1004). A companion that is attacked shouts
GEN_I_WAS_ATTACKED silently (k_hen_attacked01); the other party members hear
it, move to the attacker and fight it once they can see it (tests/shouts.rs).
The ranges are a guess (the install does not state them): whisper 3 m, talk
and silent talk 20 m, shout and silent shout 40 m (the Neverwinter Nights
values for the same nwscript constants). Speech still emits WorldEvent::Speak
for the bark display. A knocked-out creature hears nothing.
Not modelled: "**" wildcards in listen patterns (GetMatchedSubstring) and the
choice of the highest number when several patterns match. Checked: no shipped
script (638 .nss sources in the install) uses "**" or GetMatchedSubstring; every
SetListenPattern call passes a literal GEN_* name.

## Results (Aspyr 1.0.2)

- `k2world fight 102per`: the Exile (unarmed, 30 hp) against Peragus mining
  droids (blaster, faction Hostile_1). The droids notice the player through
  k_def_percept01 and k_ai_master and open fire, and two more join in from
  their own perception. The player wins after about 10 s. 0 VM errors.
- A fight sweep over all 82 modules: 72 have creatures (37 with natural
  hostiles), all 72 run with 0 VM errors and 0 unimplemented calls, and 68
  end with a death. With pathfinding the same 4 still time out, and they
  are genuinely unreachable: 201TEL BayControl and 305NAR GenoDisc stand
  3-4 m above the attacker with no walkable route, 003EBO DrdParts has no
  reachable spot within 3 m, and 001EBO is the plot-protected T3-M4 (min 1
  hp) vs HK-50 pair. The approach now fails cleanly (no path: 2 s straight,
  then stand; OnBlocked after 4 s) and each run takes under 0.5 s.
- `k2world all --seconds 60` is unchanged: 0 panics, 0 VM errors.

## Not modelled yet

Talents (feats, force powers, grenades: GetCreatureTalentBest returns an
invalid talent, so the AI uses plain attacks), saving throws, damage types
and resistances, shields and other effects beyond damage/heal/death,
attacks of opportunity, flanking, experience, loot generation on death,
weapon switching (ActionEquipMostDamaging* are no-ops), and pathfinding.

## Hostiles that never attacked (fixed)

Symptom: in play, hostile droids stood still while the player walked up to
them; the user's Peragus Administration Level save never got attacked.

Cause: perception and the AI scripts disagreed about what "seen" means.
Our OnNotice sweep and the GetNearestCreature PERCEPTION_SEEN search only
tested distance (20 m), while GetObjectSeen also needed line of sight. A
droid noticed the party through a wall, k_ai_master 1002 shouted and queued
GN_DetermineCombatRound, whose target pick requires
`GetIsObjectValid(oClose) && GetObjectSeen(oClose)`, found nobody and
returned without an ActionAttack. The notice is an edge (seen set), the
heartbeat (k_ai_master 1001) only plays ambient animations, so nothing ever
re-triggered the droid: it kept the player in its `seen` set and idled for
good, and the state was written into saves (save-0001 had droids with
local bools 90 and 94 set and an empty action queue next to the player).

Fix:
- `World::can_see(observer, other)` (combat.rs) is the one visibility test:
  perception range, not hidden by stealth, line of sight at 1.5 m eye
  height. OnNotice (tick.rs run_perception for the party, combat.rs
  perceive_hostiles for everyone else), the PERCEPTION_SEEN criteria and
  GetObjectSeen/GetObjectHeard all use it.
- `renotice_due`: an observer (hostile creature or party member) that still
  sees an enemy but has an idle queue (empty, or only a Follow), is commandable
  and not in combat gets a fresh OnNotice every 3 s (not during conversations).
  This recovers saves made mid reaction and notices whose delayed combat round
  found no target.
- Perception range now comes from the UTC PerceptionRange (ranges.2da rows
  8..13, primaryrange; the 250 player row is capped at 35 m), default 20 m.

Leader down: when a party member falls (`party_member_down`, party.rs), a
fallen leader passes control to the next standing member (LeaderChanged); with
nobody standing `WorldEvent::PartyDefeated` fires. `player_controlled()` is
false for a fallen leader. kotor2 (gameover.rs) shows the original gameover_p
screen 3 s later:
load last saved game, load list (saves.rs opens it), main menu. Test hooks:
`K2_NEAR_HOSTILE=DIST` (leader placed DIST m from the nearest hostile with a
clear line, idle), `K2_KILL_LEADER=SECS`, `K2_GAMEOVER_CLICK=BTN_LASTSAVE|BTN_QUIT`.

Verified: tests/hostiles.rs (an idle leader 10 m from an awake hostile is hit
within 15 s, also when the leader is already in its seen set; the leader can
die and PartyDefeated fires, ticking on over the corpse is clean); the
kotor2 binary loading save-0001 with K2_NEAR_HOSTILE=10 logs droid hits on
the leader 2 s after placement; K2_KILL_LEADER shows the game over screen
and BTN_LASTSAVE reloads the save. `k2world fight 102per` and
`k2world all --seconds 60` unchanged (0 panics, 0 VM errors).

## Combat state after the last hostile dies

`in_combat` used to hold for the full 6 s linger after the last attack or
hit no matter what was left alive, and the linger is only counted in world
time, so it never runs down while the game is paused or a menu is open. Now
the full linger only holds while a living hostile can still see the creature
(or it can see them); with none left, combat ends 2 s after the last attack
(`COMBAT_TAIL`, `World::in_combat`). Test: tests/hostiles.rs
`combat_ends_a_few_seconds_after_the_last_hostile_dies` (leader plus Kreia,
every hostile killed at once, no party member in combat within 4 s).
kotor2 logs "leader enters/leaves combat" with the living creatures still in
combat (gameover.rs `combat_log`); there is no combat music or combat HUD
state in kotor2 yet, and `save_blocked` does not look at combat.

## Mining droid accuracy (item properties on worn gear)

The Peragus mining droids wear the creature hide g_i_crhide015: itempropdef
19 DecreaseAbilityScore (Dex -5, iprp_neg10cost) and 41 ToHitPenalty (-5).
combat_stats only read AC from non-weapon items, so droids fought at their
UTC Dex 8 and BAB 1 (attack +0 against the leader's Defense 12: 45% to hit,
1d8-1 per blaster shot). Now Ability (0), DecreaseAbilityScore (19),
AttackBonus (38) and the attack penalties (8, 41) on any worn non-weapon item
apply (`CombatStats::gear_attack`, adjusted abilities feed Dex mod, Defense
and ability-based attack/damage). Droids end up at attack -8/-9 and Defense
5-6. Also fixed: `WeaponInfo::lightsaber` was true for every baseitem with
damageflags 4096 (blasters too: energy damage), so blasters ignored door
Hardness; it now means a lightsaber base item.

`cargo run --release -p k2-world --example duel -- MODULE CLASS RUNS COUNT`
runs seeded fights of a level 1 PC (quick_spec, class 0 Soldier, 3 Jedi
Guardian; unarmed unless WEAPON=template) against the COUNT hostiles nearest
the first one, the PC attacking whatever is engaged. 101per, 50 runs, 2
droids, Soldier: before 50% wins, 9.0 HP lost of 12, droids hit 129 of 336
swings; after 100% wins, 1.4 HP lost, 17 of 329. Guardian: 36% / 10.1 HP /
148 of 387 before, 100% / 1.8 HP / 20 of 425 after. 102per: 98% wins, 2.7-3.0
HP lost. Not modelled: difficultyopt.2da (Normal is multiplier 1.0, so no
effect at the default).

## Knocked out and getting up (party members, party.rs)

Sources (dialog.tlk and the shipped scripts, read directly):

- TLK 41883, the first-time hint: "One of your party is down! Party members who
  go down in combat will be revived at the end of the battle. If your entire
  party goes down the game is over." This is the only general statement of
  the rule. It gives no HP amount.
- TLK 48444 (Spirit, Atton): "When knocked unconscious there's a chance he
  will get back up ... provided someone else in the party is still standing.
  If he returns to the fight he will come back with 10% of his vitality
  restored." Fighting Spirit gives 20% (TLK 125694), Heroic Resolve 30%
  (TLK 125696). Only the Atton feats carry a percentage.
- TLK 48515 to 48517 (Revitalize): "fallen" allies are brought "back to
  consciousness" with 5%, 5% and 10% of their total vitality. The scripts do
  not use these percentages: k_inc_force.nss resurrects with EffectResurrection()
  and then heals a fixed 10 (Revitalize), 25 (Improved) and 50 (Master); its
  comment (FMP #4893) reads "They want it to do fixed heal amounts instead of
  percentage" (two comment lines, k_inc_force.nss 5127-5128).
- k_inc_force.nss Revitalize treats GetCurrentHitPoints < 1 as unconscious and
  skips droids (RACIAL_TYPE_DROID).
- a_attonspirit.nss is Atton's OnDeath hook (ScriptDeath here). It revives him
  8 s later with the feat's percentage, if he holds Spirit and another standing
  party member is in combat.
- k_inc_utility.nss, UT_RestorePartyToOneHealth: its block comment (lines
  2659-2660) is two lines, "Restores all party mambers to 1 hp if" and
  "tempoarily dead" (a line comment at line 190 reads "restores all party
  mambers to 1 hp if tempoarily dead"). Its callers are
  UT_JumpPartyToObject, UT_JumpPartyToLocation,
  UT_NPC_InitConversation and UT_TeleportWholeParty, all in k_inc_utility.nss.
  None of those four is called from a shipped script, so no shipped script
  reaches the 1 HP restore. It is not evidence for the get-up HP.
- Droids: Revitalize skips them, and repair kits fix "a droid that has been
  damaged in combat" (TLK 5806, 31802, 31803). Nothing sourced says whether a
  droid at 0 HP is knocked out and revived after the battle. This build treats
  droids like everyone else; that is a choice, not a finding.

Implemented as:

- At 0 HP a party member (not min1hp, not plot) gets `CreatureState.down =
  true` and HP 0; `dead` stays false. Its queue clears, it takes no orders
  (push_action), it is not an attack target (is_dead_creature), it takes no
  damage except EffectDeath, and it does not follow while down. Events:
  `KnockedDown { object, by }`, then `GotUp { object }`. ScriptDeath is queued
  on the knock-down so Atton's spirit script can run.
- EffectDeath (`harm(.., lethal = true)`) kills outright: dead, `slain`, and
  no longer down, a party member included, knocked out or not, except a min1hp
  or plot creature (see below). Died is
  emitted and kill() runs once. A knocked-out member killed this way does
  not queue ScriptDeath or call party_member_down again (both happened at the
  knock-down), so PartyDefeated is reported once.
- A standing min1hp or plot creature hit by EffectDeath stays at 1 HP. A
  knocked-out min1hp or plot creature stays down at 0 HP under EffectDeath (no
  GotUp, not dead): min1hp and plot hold a standing creature up, they do not
  stand a fallen one up. The battle-end revive still raises it, at 1 HP
  (run_party_revival does not check min1hp or plot).
- A slain creature does not come back. `resurrect` returns at once for
  `slain`, so EffectResurrection does nothing on it: not Revitalize's, not
  the Recovery Stim's, not Atton's Spirit revive (a_attonspirit runs 8 s
  later and finds him still dead). Choice, not sourced: the shipped scripts
  only say EffectResurrection revives dead creatures; that a scripted
  EffectDeath is final is inferred from its name. Revitalize on a slain
  companion therefore does nothing.
- `party_member_down` (also for real deaths): a fallen leader hands over to the
  next standing member (LeaderChanged, via `hand_over_if_leader_down`); nobody
  standing emits PartyDefeated.
- A raise (get_up, resurrect) with the leader still down hands over the same
  way (LeaderChanged). So a member raised by a heal or a resurrection while the
  party is defeated takes control. Only the battle-end revive refuses to raise
  anyone while the party is defeated; scripted raises do not.
- `run_party_revival` (each tick, from run_combat): while a standing member is
  in combat nobody gets up. Once no standing member is in combat, each downed
  member gets up after GET_UP_DELAY = 2 s. in_combat ends COMBAT_TAIL = 2 s after
  the last attack or hit once no living hostile is aware of the creature, so
  the get-up comes about 4 s after the last hit. Not while the party is
  defeated. The battle-end get-up uses GET_UP_HP = 1, for min1hp and plot
  members too.
- EffectResurrection(n) (`resurrect`): HP = max_hp * n / 100 in integer
  division (the result truncates), with n clamped to 0..100. A party member
  whose percentage HP is 0
  stays down at 0 HP (a dead one goes down). Anyone else, or a party member
  whose percentage HP is above 0, stands with max(HP, 1) and GotUp. So
  Revitalize (resurrect 0, heal 10) ends at 10 HP, Improved at 25, Master at
  50; resurrect 0 plus heal 1 ends at 1 HP.
- EffectHeal on a downed creature that ends above 0 HP wakes it at that HP.
  EffectHeal does nothing on a dead creature: a heal does not clear `dead`, so
  it would leave a dead creature with HP.
- Following: party_follow (nav.rs) walks a standing non-leader back to the
  leader when idle and skips a downed one.
- Saves: a party member saved dead at 0 HP and not slain (every save from
  before this change, e.g. HK-47 in the user's saves) loads knocked out
  (`settle_loaded_party`), and a fallen leader on load hands over. Nothing is
  written back to disk.
- kotor2: KnockedDown plays the die clip and holds the dead loop (animate_creatures
  treats `down` like `dead`); GotUp plays getupdead (getupdead1 from dead1),
  falling back to cgustandb, then idle. Corpse markers stay off knocked-out
  companions.
- Downed creatures do not regenerate: the HP regen (regen.rs run_regen, in and
  out of combat) skips `down`; pinned by a_downed_companion_does_not_regenerate
  in tests/party_revival.rs.
- Level-up (progression.rs) refuses the dead and the knocked out before any
  pick is spent, since the gain would raise a dead creature's HP or give a
  downed one HP without get_up.

Not verified against the original engine:
- The 1 HP for the battle-end get-up: no source gives a number. The only
  restore-to-1 helper is unused by the shipped scripts, and Spirit's 10% is the
  only percentage that reaches a revived companion.
- Whether control returns to the original leader after a revive: it does not
  here (the new leader keeps control, the revived member follows).
- Whether a downed companion is still hit in the original (we ignore hits) and
  whether hostiles pick other targets (they do here).
- The get-up delay (2 s after the battle) and the animation timing.
- Droids (see above).
- Whether a scripted EffectDeath is final (see "A slain creature does not come
  back").
- Spirit: the a_attonspirit path is covered by tests in tests/party_revival.rs
  but has not been played in the real game.

Tests: tests/party_revival.rs covers: a knocked-out creature takes no orders
(pins the push_action guard in lib.rs); a companion knocked out in a fight gets
up within 8 s of the battle ending and follows the leader again; a fallen leader
hands control over and gets up; the whole party down is game over and the
battle-end revive raises nobody; a member raised by a heal while the leader is
down takes control; a downed min1hp or plot companion gets up at 1 HP at the
battle end; a standing min1hp or plot companion stays at 1 HP under a hit and
under EffectDeath; a save with a 0 HP dead companion loads knocked out and gets up; a
slain companion stays dead across a save; EffectDeath on a downed companion
makes it slain and it stays dead (effect_death_kills_a_knocked_out_companion_for_good);
EffectDeath on a standing Atton and on a downed Atton is final (dead after 14 s,
the Spirit revive does not bring him back); EffectDeath on a downed min1hp
companion leaves it down; killing a downed member of a defeated party reports
PartyDefeated once; a heal does not raise a dead companion;
EffectResurrection(0) plus EffectHeal(1) gives 1 HP and EffectResurrection(0)
plus EffectHeal(10) gives 10, and a percentage stands the companion up at that
share; a downed companion does not regenerate; Atton's Spirit gets him up about
8 s after the knock-down with 10% HP. Old hostiles.rs test
the_only_party_member_can_fall_and_the_party_is_defeated and the gameover test
dead_leader_blocks_saving_and_last_save_skips_dead_states (in
crates/kotor2/src/gameover/tests.rs) were updated for the knock-out state.
Unit tests in crates/k2-world/src/party/revive_tests.rs
cover what needs the event queue or the routine table: EffectDeath on a downed
member queues ScriptDeath once and reports no party change; a dead or
knocked-out creature cannot level up.

## Regeneration (regen.rs)

Sources (all in the install unless marked secondary):

- regeneration.2da, read by row label (value column): timeperhp 6.0,
  outofcombathpbase 1.0, incombathpbase 0.0, conmodbonus 0.3,
  skillrankbonus 0.1, fpregentime 90.0, outofcombatfpbase 1.0,
  incombatfpbase 0.1, wismodbonus 0.0, roomforcebonus 0.075.
  regeneration_e3.2da is the E3 build's table (timeperhp 1.0, out 3.0, in
  0.5, conmod 0.15, fpregentime 60, incombat FP 0.2, wismod 0.075). Not used.
- nwscript.nss: EffectRegenerate(nAmount, fIntervalSeconds) ("amount of
  damage to be regenerated per time interval"), EffectFPRegenModifier and
  EffectVPRegenModifier (percent), DisableHealthRegen ("turns off the innate
  health regeneration that all party members have"). No shipped script calls
  DisableHealthRegen.
- a_swapimplant.nss (its header: implant swapping for Canderous): the
  regeneration implant case applies EffectRegenerate(4, 6.0) to OBJECT_SELF.
  The shipped body never creates a hide; the only G_I_CRHIDE018 creation is
  in the commented-out SwapImplant block.
- g_i_crhide018 (UTI, not tied to a named creature in any source read): item
  property Regeneration (35) with value 4. A creature wearing it and also
  carrying the implant effect would count both (4 + 4 per 6 s); nothing read
  so far shows that happening.
- k_inc_force.nss: Battle Meditation (FORCE_POWER_BATTLE_MEDITATION_PC, and
  its Improved and Master forms) applies EffectVPRegenModifier(50) for 20 s.
- dialog.tlk: 32068 describes spell row 53 SPECIAL_ABILITY_COMBAT_REGENERATION
  (Canderous's ability; "constantly regenerates one vitality point every six
  seconds"); the ability is not implemented. 38092 ("Force Points regenerate
  slowly while your character is fighting, but return quickly after combat."),
  48100 and 48319
  (feats 143 and 196, Level 4), 697 and 718 (property names).
- feat.2da: 143 REGENERATE_FORCE_POINTS, 196 REGENERATE_VITALITY_POINTS.
- itempropdef.2da: 35 Regeneration and 54 Regeneration_Force_Points, both on
  cost table 2 (iprp_meleecost: value = points). 35 sits on hides
  g_i_crhide003..018, u_l_crys_24/25, e_imp1_08, e_imp2_03, d_t3m4_02 (value
  3), a_gloves_24, a_helmet_18, a_robe_26; 54 on the a_robe_* and a_helmet_21/30
  set and on lightsabers (values 1 to 4 across the shipped UTIs).
- Secondary, web: StrategyWiki K2 pages (Force powers, Attributes) as quoted
  in search results: Force points refill in 90 s out of combat and 900 s in
  combat; feat 143 gives x1.25 out of combat and x3.5 in combat; the
  Constitution table (in combat -0.3 at CON 8, out 0.7; in combat 0.3 per
  modifier point, out 1 + 0.3 per point). The StrategyWiki page itself
  returned HTTP 403, so these were not read first hand. DeadlyStream "No
  Health Regeneration" (file 2530) overrides regeneration.2da and says
  "You can still gain regeneration through items or the Regenerate Vitality
  feat".

Rules as implemented (regen.rs; run_powers calls run_regen once a second):

- Vitality, innate, party members only: (base + 0.3 * CON mod) / timeperhp
  points per second, base 1.0 out of combat and 0.0 in combat, never below 0.
  CON 10 out of combat is 1 point per 6 s; CON 18 is 2.2 per 6 s. In combat
  only the Constitution term is left, so CON 10 and below do not heal in
  combat. CON comes from combat_stats (gear and effects included).
- Item property Regeneration (35) on any equipped item in slots 0-17: value
  points per 6 s (timeperhp), any creature, in and out of combat.
- EffectRegenerate(n, s): n points per s seconds, any creature, in and out
  of combat (the combat part is a guess; the implant script has no combat
  condition).
- EffectVPRegenModifier(p) scales the total vitality rate by (1 + p/100).
  Battle Meditation gives +50% for 20 s.
- Force, any creature with a Force pool (guess for NPCs: the table and the
  innate rule name no NPC case): max_force * base / fpregentime per second,
  base 1.0 out of combat (a full pool refills in 90 s) and 0.1 in combat
  (900 s). Property Regeneration_Force_Points (54) adds value points
  per 6 s while below the maximum (secondary: the feat/item text as quoted
  above). Feat 143 (level 4 Jedi) multiplies the base by 1.25 out of combat
  and 3.5 in combat (secondary). EffectFPRegenModifier(p) scales the total.
- Wisdom: wismodbonus is 0.0, so Wisdom does not change the rate (the term is
  wired so a table change takes effect).
- No regeneration while dead, knocked out (down), or while a conversation
  runs (guess for the dialogue case). Vitality and Force never pass their
  maximum. Fractions carry between seconds in CreatureState.regen_carry
  (saved, serde default).
- DisableHealthRegen(TRUE) switches off the innate vitality rate of the party
  until FALSE (PowersState.innate_hp_regen_off, saved). Item, effect and
  feat regeneration are not affected.

Not applied, or guesses:

- skillrankbonus (0.1) and roomforcebonus (0.075): nothing in the shipped
  scripts or text says what they scale. Not applied.
- Feat 196 (Regenerate Vitality Points, level 4): the text says "increased
  healing rate" and gives no number. Not applied (it does nothing yet).
- Spell row 53 SPECIAL_ABILITY_COMBAT_REGENERATION (TLK 32068, one point per
  6 s while conscious): not implemented.
- Possible double count: a creature wearing item Regeneration 4 that also
  has the implant effect counts both (see the g_i_crhide018 note above).
- Droids: the innate rule covers all party members, so HK-47 and T3-M4 get
  it too (guess; repair kits are still their normal repair).
- The item units (points per 6 s) match the implant effect's interval (4 per
  6 s on both); the Force item unit (value per 6 s) is the secondary source's.
- Pausing: menus stop the world tick; the dialogue pause is a guess.

Tests: crates/k2-world/tests/regen.rs (8 tests on 003ebo and 102per):
rate against CON 18 down to 2 (clamped at 0), Force 90 max refills 1 per s
and caps, 60 s of ticks gain the rate, the in-combat rates on Peragus, the
g_i_crhide018 and a_robe_23 items, EffectRegenerate and the timed Battle Meditation
modifier, knocked-out members stay put, and DisableHealthRegen.

## Game over and saving

`World::save_blocked` / `exit_save_blocked` refuse with "the party is dead"
while the leader is dead or the party is defeated (every member dead or
knocked out), so quicksave, autosave, the timed autosave and the restart/exit
save can no longer capture the game over state (the timed slot every 120 s was
the likely way a dead leader got into the newest save, which "Load Last Saved
Game" then reloaded). The button (gameover.rs `last_good_slot`) picks the newest
of save-*, quicksave and autosave whose leader is standing (not dead, not
knocked out; it loads each candidate to check), then falls back to the rolling
timed/restart slots, and skips slots with a dead or knocked-out leader. The
slots in the user's saves dir were checked: none has a dead leader. Test:
`dead_leader_blocks_saving_and_last_save_skips_dead_states`
(crates/kotor2/src/gameover/tests.rs).
Binary check (copy of the saves dir via K2_SAVE_DIR): K2_KILL_LEADER, then
K2_GAMEOVER_CLICK=BTN_LASTSAVE logs the slot loaded and "game over cleared:
leader alive with 12 hp".

## Bashing doors and placeables (bash.rs)

- Bashable: door not Plot, not Static, not NotBlastable; placeable not Plot,
  not Static. HP from UTD/UTP (current = CurrentHP, max = max(HP,
  CurrentHP)), Hardness from the blueprint. Kept in DoorState/PlaceableState
  `hp` (saved).
- Attacks on objects always connect (no Defense). Damage = weapon dice +
  extra dice + str mod (melee) + damage bonus, minus Hardness, floored at 0.
  Lightsabers and weapons with the DoorCutting property (itempropdef 64)
  ignore Hardness: that is the plasma torch (`plasmatorch.uti`, Peragus).
- The plasma torch (DoorCutting) does not accumulate damage: its first swing
  deals the object's remaining HP, so a bashable door or locked placeable is
  cut open after one attack action (OnMeleeAttacked, OnDamaged, OnDeath and
  the door open still fire through destroy_object_by_damage). Plot and
  NotBlastable doors stay unbashable. Lightsabers keep the HP rule
  (hardness ignored, weapon damage per hit).
- One swing per attack slot of the 3 s round; OnMeleeAttacked per swing,
  OnDamaged per hit, OnDeath at 0 HP. A destroyed door unlocks and opens for
  good (walkmesh to Open1); a destroyed placeable unlocks.
- Approach: the attacker does not walk at the object origin (it sits inside
  its blocker). bash.rs picks a standing point about 1.2 m out on either side
  of a door (8 points on a 1.4 m ring for a placeable) that has ground and a
  walkmesh path from the attacker, takes the shortest, and follows the nav
  path there (cached per attack); it swings once within reach on the same
  side as that point. No reachable point, or 2 s without progress, aborts:
  the whole action queue is cleared, the party member barks "I can't reach
  that." and control returns. A movement key (play.rs) also drops the leader's
  attack on an object (`World::is_bashing`), in or out of combat.
- GetCurrentHitPoints / GetMaxHitPoints answer for doors and placeables.
- Data: the 001ebo prologue doors are all Plot + NotBlastable (Security,
  mines and scripts open them). Peragus doors (sw_door_per00x: 60 HP,
  Hardness 20) shrug off normal weapons and open on the first torch swing.
- kotor2: E/click on a locked bashable door or container attacks it; the
  target display shows its HP. Test flag `--autoattack-door TAG`.

## Projectiles (projectiles.rs in k2-world and kotor2)

World side (`World::emit_bolt`, `emit_spell_projectile`): one
`WorldEvent::Projectile` per ranged attack (before the outcome event) and one
per projectile spell cast (at the cast start). The event carries the model,
the hook it leaves from, the landing point, when it leaves (`launch`), how long
it flies (`flight`), whether it is thrown in an arc, and how it ends: Impact,
Miss, Deflect or Detonate. Deflected attacks also send `AttackDeflected` after
the `AttackMissed` they keep. Client side: kotor2/src/projectiles.rs draws it;
sfx.rs uses the same launch and flight for the impact sound.

Data, Aspyr 1.0.2:
- Bolt: the weapon that fired it (the attack's own main or off-hand weapon).
  baseitems `ammunitiontype` (1 Blaster, 2 Ion, 3 Disruptor, 4 Sonic,
  5 Bowcaster) selects an ammunitiontypes.2da row: `model` is the bolt
  (w_laserfire_r, _b, _w, _s, _bc), `muzzleflash` is v_muzflash_01, plus the
  shot and impact sounds. All 14 ranged baseitems resolve (tests/projectiles.rs).
- The bolt model has a `zap` emitter (Fx_laser_01, Explosion, Motion_Blur,
  Lighten) and an AuroraLight01. Its travel01 clip fires `detonate` at 0 s, so
  the emitter is a burst. The client draws streaks instead of running it.
- Weapon models carry `bullethook` (the muzzle) and `muzzlehook`.
- Repeating blasters: the data has no shots-per-round column. Repeating_Blaster
  has numdice 2, which is damage. One bolt per attack, like every other blaster.
- No blaster clip (b5, b6, b7, b9) has an event, so the shot is at a fixed
  BOLT_LAUNCH (0.25 s into the attack). That time is a guess.
- Spells: 36 spells.2da rows have proj 1, all projspwnpoint `throw`. Thrown
  grenades (projtype grenade) are spells 87-98 and 186-187. Launched grenades,
  rockets and darts (projtype homing, projorientation target) are 185,
  190-199 and 246-256. The projmodel is the item model in flight (for example
  w_fraggren_001). No Force power has a projectile row. The saber throw (proj
  0, throwsab) and lightning (projtype linked, proj 0) are not projectiles here.
- Explosion: k_sup_grenade.nss runs a visualeffects.2da row per spell (nVFX,
  3003 fragmentation to 3011 ion). The row gives `imp_root_m_node` (for example
  v_grnfrag_fnf, with an `impact` clip and detonate emitters) and `soundimpact`
  (cb_gr_fragment). The VFX id is a row label, not an index: the table has gaps
  (167 data rows, last label 9015; test visualeffects_ids_are_row_labels_with_gaps),
  so blast() looks the row up by label. Darts, 194 and the Kor grenades have no
  VFX row, so they end with a flash only.
- Thrown grenades: throwgren (1.5 s, no events). The cast anim "throw" is also the
  saber throw's, so progress_ui picks throwgren for grenade rows only.

Timing and outcome:
- Bolt flight: distance from the weapon hook (feet + 1.1 m) to the target's
  chest (feet + 1.0 m) over BOLT_SPEED (60 m/s, tuned by eye, not measured).
  The damage still lands when AttackStarted fires (unchanged), so a distant
  target can take its damage a fraction of a second before the bolt arrives.
- Miss: the bolt flies on to 6 m past the chest, 0.8 m to one side. The side is
  chosen by object id parity, so the combat RNG is not used and the duel
  numbers do not change.
- Deflect: the bolt stops 0.5 m in front of the chest on the shooter's side,
  then the client bounces it back toward the shooter in 0.4 s, for the lesser of
  12 m and the distance back to the shooter's hook. Sound: the target's weapon
  clash cell.
- Hand: the event's `hook` is the hand it leaves from (`rhand`, or `lhand` for an
  off-hand blaster, which the attack resolved as `offhand`). Its ammunition is the
  attacking weapon's own (WeaponInfo base item), so an off-hand bolt is the
  off-hand weapon's bolt, with its muzzle flash. A bolt leaves the `bullethook`
  of the weapon held in that hand.
- Sounds: the event's `ammo` is the ammunitiontypes row of the weapon that fired
  (its hand's weapon). sfx.rs plays that row's shotsound at the launch and its
  impactsound at the landing, so an off-hand blaster sounds like its own
  ammunition (Blaster_Pistol: cb_sh_blast1; Ion_Blaster: cb_sh_medbl1).
- Fake casts (ActionCastFakeSpell*) run the animation only: nothing is thrown.
- Grenade: released at conjtime (0.17 s after the cast starts) and lands at the
  target's feet with the impact script: flight = casttime / 2 (0.665 s), so
  landing = impact = 0.835 s (tests/projectiles.rs checks both).
- Rockets and darts fly straight over the same time as a grenade.

Presentation of a ranged hit (presentation.rs): the world still applies the hit at
the attack tick (the rules and the AI rely on that), but the client holds the
hit's presentation until the bolt of its shooter lands: the damage numbers, the
miss text, the pain voice, the attack's log line, and a death, whose dying pose
animate_creatures keeps until then. The impact sound and the flash follow the
bolt on their own (sfx.rs takes the outcome from the Projectile's end). Only
bolts hold: melee, spells and grenades show as they happen. Not held: the health
bars, which read the world's state and drop at once.

Not modelled: the travel emitter of the bolt model; shield-hit sparks
(ammunitiontypes shieldhit); a deflection won by 10 sending the bolt back at
the attacker to hurt them; the lightsaber's damage (see below); lightning beams;
the grenade throw sound (cs_grentoss.wav). The arc height (25% of the throw
plus 0.8 m, at most 4 m), the bolt light (4000, range 3) and the flash sizes
are guesses for the user to judge in play.

Tests: `cargo test -p k2-world --test projectiles` (install needed; skips
without one) covers every ranged weapon and every projectile spell's model
and explosion, one bolt per ranged attack in 101per, and the grenade timing.
`cargo test -p kotor2 projectiles` covers the flight, bounce and miss logic
with MinimalPlugins and no assets.

## Lightsaber throw (spells 49 and 4; projectiles.rs, saber_throw.rs)

Data: spells.2da rows 49 (FORCE_POWER_LIGHT_SABER_THROW) and 4 (..._ADVANCED).
Both have castanim and conjanim `throw`, conjtime 560, casttime 1940, catchanim
`CATCH` with catchtime 500, proj 0 and impactscript k_sp1_generic. They are the
only two rows with a catchanim, which is how the world tells them apart.
Clip lengths, from the install's models through k2-mdl (Animation.length is read
from each animation header: k2-mdl reader.rs, anim_header_offsets::LENGTH; the
kotor2 saber_throw test asserts these): throwsab 0.5667 s in s_male02 (conjtime
560 ms), catchsab 0.50 s (catchtime 500 ms), and in w_lghtsbr_001 throwout 0.8 s,
taken here as the spin, and throwback at length 0.0 (a single pose). The path is
not in the data: it is ours.

The advanced throw's targets are the script's. k_inc_force.nss, as installed.
The script is not a loose file: chitin.key lists k_inc_force as type 2009 (NSS)
in BIF 9, data\scripts.bif. `target/release/k2probe cat k_inc_force.nss` reads it
through that index, and the lines below are from that output:

    2639  oTarget2 = GetNearestCreature(CREATURE_TYPE_REPUTATION, REPUTATION_TYPE_FRIEND, oTarget, 1);
    2640  if(GetIsObjectValid(oTarget2) && GetDistanceBetween(oTarget, oTarget2) <= 5.0)
    2642  oTarget3 = GetNearestCreature(CREATURE_TYPE_REPUTATION, REPUTATION_TYPE_FRIEND, oTarget, 2);
    2643  if(GetIsObjectValid(oTarget3) && GetDistanceBetween(oTarget, oTarget3) <= 5.0)

Lines 2644 to 2660 hit three targets (oTarget, oTarget2, oTarget3) when both
friends pass, and two (oTarget, oTarget2) when only the first does. So the
friends are the nearest and second nearest of the target's friends, each within
5.0 of the target. The code runs combat.rs nearest_creatures around the target
with criterion 3 (reputation) value 0 (friend), the model the combat searches use
for GetNearestCreature: a candidate qualifies when reputation(target, candidate)
is at least 90, that is, when the target regards it as a friend. The script does
not state the direction. The same call at line 948, GetNearestCreature(
CREATURE_TYPE_REPUTATION, REPUTATION_TYPE_FRIEND, OBJECT_SELF, nCnt), gathers the
party around OBJECT_SELF, and the party is friendly both ways, so line 948 does
not settle it. The direction is the project's model, pinned by the integration
test the_advanced_throw_follows_the_targets_opinion_of_a_neighbour (an asymmetric
pair), not checked in the game. nwscript.nss declares GetNearestCreature(
nFirstCriteriaType, nFirstCriteriaValue, object oTarget=OBJECT_SELF, int nNth=1,
...), which confirms the argument order.

The TLK text for 4 (1204) says the first target must be at least 5 m away and
each additional target must be within 5 m of the previous, with no friend
test. The code follows the script: the friends are measured from the target.

The plain throw hits the target only. The thrower's right-hand lightsaber (its
item model, not a crystal) is the one thrown; with no saber in hand the cast
does not start (the TLK text says "Requires Lightsaber"; reading that as a cast
gate is ours).

Timeline (world, `emit_saber_throw`): leave the hand at conjtime (0.56 s); one
leg per stop, sharing the time up to the impact at conjtime + casttime / 2
(1.53 s); back to the hand by the end of the cast (2.5 s). The saber catches
at 2.5 s, so the catch clip (catchsab) runs from 2.0 s (catchtime before). The
client hides the held saber while it is out and restores it when it is back.

Damage (the rule text is TLK 1248 for 49 and 1204 for 4, "Universal Power"):
the throw always hits, for one d6 per two levels of the thrower, and each stop
applies its own hit when the saber reaches it. The plain throw hits the target;
the ADVANCED throw also hits its friends within 5 m of the target (the script's
rule; the TLK text says within 5 m of the previous target). The world runs these
hits itself (CastAction.saber_hits in powers.rs, saber_hit in projectiles.rs);
the impact script's EffectLightsaberThrow stays a recorded effect on the caster
and does nothing. Guesses, marked in code: one die at level 1 (the text gives
none), no strength or enhancement added, and the 5 m minimum checked when the
cast starts (a closer target, or no saber in hand, makes the cast fail). A cheat
cast skips the 5 m check and not the saber: without a saber it does not start.

Blade: the lightsaber's blade is the MDL Saber node. Its positions, UVs and
normals are saber_verts, saber_uvs and saber_normals (176 each); the base mesh's
12 faces (over vertices 0-33, mostly slivers) are not used. In the bind pose only
two of the 22 pieces of 8 vertices have area: 0-7 (x from 0 to +0.1 m) and 88-95
(x from 0 to -0.1 m), each a three-quad strip from z -0.043 to 0.98 m. The other
pieces repeat the x = 0 column and have none. The strip pattern is reone's
kSaberPieceFaceIndices (seedhartha/reone, src/libs/graphics/format/mdlmdxreader.cpp,
GPL-3). Its winding gives the saber normals on the front piece; the back piece is
reversed to match. The second Saber node has the same mesh turned 90 degrees
about z (w_lghtsbr_001: both at z 0.18, local orientations identity and 90 degrees
about z, from MdlNode position and rotation), so the two make a flat cross about
0.2 m wide and 1 m long. The offset 88 is half of the 176 verts. model/tests.rs
checks the strips, that every other saber vertex has x = 0, and the two poses
(lightsaber_blade_is_two_strips_of_the_saber_vertices and
the_two_saber_nodes_cross_at_the_hilt). The pattern is checked against the
install's data, not in the game. The glow sheets (34 vertices) are ordinary meshes.

Guesses: the return goes to the hand's position at launch, and the catch timing
(lead equal to catchtime).


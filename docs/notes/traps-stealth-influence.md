# Mines, traps, stealth, influence

Code: crates/k2-world/src/{traps,stealth,influence}.rs, the Bevy side in
crates/kotor2/src/systems_fx.rs, plus action bar entries in progress_ui.rs.
Tests: crates/k2-world/tests/systems.rs (7, against the install),
crates/k2-world/tests/trapped_objects.rs (trapped doors and containers,
locked placeables, routines, stealth toggle, cutscene and attack breaks,
set/recover round trip, the UTD/UTP trap block from synthetic GFF) and
crates/k2-world/tests/bash.rs (bashing a trapped door open).

## Traps and mines

- Data: UTT TrapFlag, TrapType (traps.2da row), TrapDetectable,
  TrapDetectDC, TrapDisarmable, DisarmDC, TrapOneShot, Faction,
  OnTrapTriggered, OnDisarm. Added to k2_data::Trigger. The state lives on
  the trigger object (`Object.trap`), so saves and module memory keep it.
- 252 trap triggers in 21 modules (102per 12, 901mal 56, 232tel 36, ...).
  All shipped ones are Faction 13 (Trap) and mostly traps.2da mines run by
  k_trp_generic.
- traps.2da: label, trapscript (k_trp_generic for all mines, k_kor_rocks*
  for the Korriban rockfalls), setdc, detectdcmod, disarmdcmod, trapname,
  resref (the trap kit UTI, for Recover), model (v_mnfrag, v_mnstun,
  v_mnplasma, v_mngas), explosionsound (cb_gr_*).
- Factions: repute.2da row Trap (13) is 50 for everyone, but the Player and
  Friendly_1 rows have 0 toward Trap while enemy rows have 50. A trap goes
  off when the entering creature's row is hostile toward the trap's
  faction, so GIT traps hit the party and not the enemies. For a trap as
  the source of a reputation check (k_trp_generic's
  `!GetIsNeutral(oTarget, OBJECT_SELF)`) the engine has to use the target's
  row, or no one would ever be hurt. World::reputation does that for trap
  objects.
- Mines set by a creature carry its faction (party mines are Friendly_1),
  so Hostile_1 creatures set them off. Detect/disarm DCs are the setter's
  Demolitions + traps.2da detectdcmod/disarmdcmod. GetOwnerDemolitionsSkill
  returns that rank (k_trp_generic widens the blast by 0.1 m per rank).
- Detection: every 2 s, party members within 10 m of an undetected
  detectable trap roll d20 + Awareness (skill 3) against the detect DC.
  Feedback TLK 42132 "<creature> successfully detects <mine>: awareness N
  vs. DC M" (a guess for traps, see Item property Trap below). Per creature
  (GetTrapDetectedBy); the party sees what any member detected.
- Disable Mine / Recover Mine (TLK 1529 / 1531): d20 + Demolitions (skill 1)
  against the disarm DC, Recover +5 (inferred), within 3 m. Recover gives
  the traps.2da resref item back. Failing by 5 or more sets it off. The
  creature that set a mine gets it back without a roll (inferred; any
  other party member, same faction or not, rolls; test
  another_party_member_rolls_to_recover_a_mine). Labels: "Disable Mine" /
  "Recover Mine" (1529 / 1531) with "failure" (1393) or "success" (1392).
  Recover success shows both lines: "Recover Mine: success" and the
  item-received line for the kit (inferred; both are kept).
- Item property Trap (itempropdef row 46), Subtype = traps.2da row; trap
  kits are baseitems Trap_Kit (58). Set Mine plants it 1 m in front and
  gives TLK 37251 "Mine successfully laid." Detection feedback uses TLK
  42132 ("<creature> successfully detects <trap>: awareness N vs. DC M"), a
  guess for traps. An unnamed door or placeable trap is named by its
  traps.2da name (test detection_of_an_unnamed_placeable_trap_uses_its_trap_name).
- Doors: k_trp_generic only hurts creatures (GetFirstObjectInShape with
  OBJECT_TYPE_CREATURE). The engine itself breaks doors with UTD
  NotBlastable = 0 within the blast, Plot or not. 001ebo engine_door is Plot,
  NotBlastable 0, HP 1/3, and its dialog eng_door says "You can use a mine
  to open this door. Once you plant the mine, back away from the door to
  avoid taking damage." No script detonates the mine, so a mine planted
  within 3.5 m of a closed blastable door is treated as a breaching charge
  that goes off after 4 s (inferred from that text). Test
  mine_blows_the_engine_room_door opens it this way.
- Routines: GetIsTrapped, GetTrapBaseType, GetTrapDetectable,
  GetTrapDisarmable, GetTrapFlagged, GetTrapOneShot, GetTrapDisarmDC,
  GetTrapDetectDC, GetTrapKeyTag, GetTrapCreator, GetTrapDetectedBy,
  GetNearestTrapToObject, SetTrapDetectedBy, SetTrapDisabled,
  GetLastDisarmed, GetLastTrapDetected, GetOwnerDemolitionsSkill, SpawnMine,
  DetonateMine, ActionUseSkill (Demolitions on a trap).
- ActionUseSkill(SKILL_DEMOLITIONS, trap, nSubSkill), nwscript SUBSKILL_*:
  0 disable (no constant, the default), 100 SUBSKILL_FLAGTRAP, 101
  SUBSKILL_RECOVERTRAP, 102 SUBSKILL_EXAMINETRAP.
- Flag Trap (100): needs the trap known to the user (flagged, detected by
  them, or party-known); sets GetTrapFlagged, which makes it visible to every
  creature. Examine Trap (102): a trigger trap gets TLK 1517 "This mine
  appears to be a: <CUSTOM0>" with its traps.2da name. A door or placeable
  trap gets the traps.2da name alone: no original TLK line says "trap", so
  nothing is synthesized from the mine line (test
  examine_gives_the_mine_line_only_for_a_mine). A party member stepping on
  a mine (trapscript k_trp_generic) gets TLK 1461 "You triggered a Mine!"
  (the rockfalls do
  not).
- DetonateMine (nwscript 857: the mine is scheduled to play its detonation
  once it is destroyed, and "has nothing to do with causing the mine to do
  any damage"; a script that wants damage ExecuteScripts k_trp_generic
  first): the explosion event, then a trigger mine is destroyed whatever its
  one_shot or creator flags (test detonate_mine_removes_a_creator_set_mine).
  A trapped door or placeable is only disabled, not destroyed (inferred from
  the wording; test detonate_mine_only_disables_a_trapped_placeable). A
  trigger with no trap state (HintKolto in 101per) is not a mine and is left
  alone (test detonate_mine_leaves_a_plain_trigger_alone). No trap
  script run. Door blast kept (inferred). Guess: a mine already disabled
  (SetTrapDisabled) explodes nothing but is still removed.
- Recover applies only to trigger traps (mines, GIT traps); a door or
  placeable trap has no kit to give back.
- Set, detect and recover round trip is covered by
  trapped_objects::set_detect_and_recover_a_mine.

## Trapped doors and placeables

- UTD and UTP TrapFlag, TrapType, TrapDetectable, TrapDetectDC,
  TrapDisarmable, DisarmDC, TrapOneShot and Faction are read into
  k2_data::blueprint::TrapFields (on Door and Placeable). spawn.rs puts a
  TrapState on the object (Object.trap, saved with it). GIT door and
  placeable entries carry no trap fields; the trap comes from the template.
- Detection and disarm/recover use the same code as mines (detect_traps,
  disarm_trap), with the object as the trap. A disarmed or one-shot trap
  stops being armed; the object stays.
- Triggering (inferred, nothing in the install settles it): a player use
  that opens a closed door (use_object), uses a placeable (OnUsed path,
  unlocked only, see Locked objects below), or opens an unlocked container
  (open_container) sets an armed trap off first, with OBJECT_SELF = the
  object and the object's OnTrapTriggered or the traps.2da trapscript
  (k_trp_generic). Then the open still goes through. Detection alone does
  not stop it; only a disarm does. Scripted opens (ActionOpenDoor and
  friends) do not trigger traps.
- Locked objects: a locked door fires OnClick and OnFailToOpen and nothing
  else; a locked container fires OnFailToOpen. A locked non-container
  placeable runs OnUsed and its conversation (13 shipped templates rely on
  it, e.g. 604dan knrthegg), but its trap does not go off while it is locked
  (test locked_placeable_keeps_its_trap_while_locked; the trap fires on the
  first use after unlocking). Inferred.
- Transition doors (UTD LinkedToModule): a trapped one goes off before the
  transit is requested (test trapped_transition_door_goes_off_before_the_transit).
  Inferred, as for any trapped door.
- Bashing: destroying a trapped door or placeable with damage (bash.rs
  destroy_object_by_damage) sets its armed trap off, before the door swings
  open. This happens whether or not the object is locked: the blow sets the
  trap off regardless of the lock (inferred; the code path has no lock
  check). Source: the NWN Bash page (https://nwn.fandom.com/wiki/Bash), which
  says a trap on a bashed object is set off by the blow. That was read from
  a search summary, not the full page, so the exact wording is unconfirmed.
  The KotOR engine was NOT checked for this; treat it as unverified.
  Tests: trapped_door_goes_off_when_bashed_open (bash.rs) and
  trapped_placeable_goes_off_when_bashed_open (bash.rs, locked placeable).
- A trapped door or placeable is never destroyed by its trap or a disarm
  (only trigger traps are), and its trap never blasts doors.
- Survey: `cargo test --release -p k2-world --test trapped_objects --
  --nocapture survey_trapped_doors_and_placeables` lists the shipped ones.
  No shipped UTD or UTP has TrapFlag set: the survey asserts 0 of each, so
  a change shows up as a failure. The mapping from the trap block is
  covered by trap_block_of_a_template_maps_to_trap_state (synthetic GFF).

## Stealth

- Stealth Field Generators are baseitems Stealth_Unit (88, belt slot), e.g.
  a_belt_03. With one equipped the leader gets a Stealth entry on the
  action bar (icon isk_stealth). Entering is refused in combat (TLK 1452);
  TLK 1464 / 1465 are the on/off feedback. Running is disabled while
  stealthed.
- Hidden: take-10 opposed check, Awareness + 10 vs Stealth + 10 + 5 (field
  bonus, inferred), always seen within 1.5 m. Hidden pairs feed both
  perception passes (OnNotice), GetObjectSeen and the GetNearestCreature
  perception filters, so AI scripts don't target what they don't see.
  Combat ends stealth: an attack puts the attacker in combat, so stealth drops
  on the next tick (tests stealth_breaks_when_attacking). A cutscene line
  ends stealth for the owner and the PC too, unless SetKeepStealthInDialog
  is on (nwscript: "By default, stealth is removed from anyone taking part
  in a cutscene"). Every staged node reached ends it, not only the start:
  a cutscene line is AnimatedCut or CameraAngle 4, with or without a camera
  model (dialog.rs cutscene_line; miraapt in 303nar has one such line, entry
  39, behind an Intelligence check). A plain line keeps stealth (guess:
  nwscript names cutscenes only). Tests: stealth_ends_when_a_cutscene_starts
  (101awake), stealth_ends_at_a_cutscene_line_inside_a_plain_dialog (101atton,
  entry 58 made a CameraAngle 4 line), stealth_survives_a_plain_conversation
  (101atton).
  Facing is unchanged: dialog.rs scripted_staging (AnimatedCut, or CameraAngle
  4 only when the dialog has a camera model) still decides whether speakers
  turn. Test camera_angle_line_without_a_camera_model_still_faces (facing.rs).
  The flag is not saved (nwscript: "it isn't saved"; serde skip).
  ActionUseSkill(SKILL_STEALTH) toggles it; the
  craft_call arm that used to swallow every ActionUseSkill is now Security
  only, so Stealth and Demolitions reach their handlers.
- Sneak Attack (feats 60 to 69, TLK 1459 "Sneak Attack!") is a combat
  bonus, not in stealth.rs. Not implemented; the bonus rules are not in the
  install's 2da data (unverified web sources say stealth field, stunned or
  behind targets).
- Stealth XP (K2): Get/SetMaxStealthXP, Get/SetCurrentStealthXP,
  Get/SetStealthXPEnabled, Get/SetStealthXPDecrement, AwardStealthXP (gives
  the current pool as XP); saved. SetKeepStealthInDialog is not saved (see
  Stealth).
- Look: FLAG_STEALTH in the KMaterial shader, a dithered see-through body
  with a moving shimmer band, depth-writing like the hologram look.

## Influence and alignment

- Influence per NPC index (npc.2da: 0 Atton ... 6 Kreia, 8 T3m4, 11
  Disciple), 0..100, default 50, saved. Dialog scripts a_influence_inc /
  a_influence_dec (params NPC, magnitude) call ModifyInfluence; the feedback
  is TLK 48607 / 48608 "Influence Gained/Lost: <CUSTOM0>" with the
  companion's name (its creature, else p_<npc.2da tag>).
- AdjustAlignment / SetGoodEvilValue on a party member: TLK 42441 / 42440
  "Light/Dark Side Points Gained" with the LightSide / DarkSide interface
  cue. a_givelight / a_givedark take the amount as script parameter 1.
- Feedback goes out as WorldEvent::Feedback { text, cue } into the message
  log's feedback tab; the cue is a guisounds.2da label.

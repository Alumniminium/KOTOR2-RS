# World simulation (crates/k2-world)

Headless and authoritative. The Bevy layer mirrors it; nothing in here
knows about rendering. Game coordinates (Z-up, meters).

## Using it from the presentation layer

    let mut w = World::new(install)?;
    w.enter_module("001ebo", None)?;          // fires OnModLoad, OnSpawn, OnClientEnter, area OnEnter
    loop {
        // input
        if w.player_controlled() { w.set_player_position(pos, bearing); }
        w.use_object(w.party_leader(), clicked);  // doors, placeables, creatures (talk)
        // dialog UI
        if let Some(v) = w.conversation_view() { /* v.text, v.replies */ }
        w.choose_reply(i); w.advance_conversation(); w.end_conversation(true);
        // simulation
        w.tick(dt);
        for e in w.drain_events() { /* WorldEvent */ }
        if let Some(t) = w.pending_transition.take() {
            w.enter_module(&t.module, t.waypoint_tag.as_deref())?;
        }
        // positions: w.iter() / w.get(id): position, bearing, kind, detail
        // animation: w.motion.get(&id) = Some(Walk | Run) while a scripted move ran this tick
    }

- Ids are stable for the life of the object. The player and party keep
  their ids across `enter_module`; everything else is respawned.
- Destroyed placeables and doors drop their walkmesh (`placeable_walk`, `door_walk`): scripts such as the 105PER SecurityDoor OnOpen destroy invisible `plc_dblk` blockers, and a leftover collider kept the open door shut.
- `World::walk` is the area WalkMesh: room WOKs plus placeable PWKs plus door
  DWKs (door open/close toggles them). Use it for player movement too.
- `WorldEvent`s: ModuleEntered, ObjectSpawned/Destroyed, PlayAnimation
  (looping flag + duration), DoorChanged, PlaceableChanged,
  ConversationStarted/Ended, Bark, Speak, DebugText (AurPostString),
  FadeOut/FadeIn, Movie, PlaySound, SoundObject, VisualEffect, Journal,
  TransitionRequested, OpenStore, ShowGui (partyselection, galaxymap,
  upgrade, endgame, ...).
- `ConversationView`: text with tokens substituted and {director notes}
  stripped (the game never shows them; many cutscene nodes are only notes,
  so their text is empty), speaker/listener ids, VO and sound resrefs, raw
  Delay, paused flag, replies (empty text = "[Continue]"), and `boxed` (a
  door's message box, see Door use).

## Engine behaviour as implemented

- Script context per run: GetEnteringObject, GetExitingObject,
  GetClickingObject, GetLastUsedBy/OpenedBy/ClosedBy, GetLastSpeaker,
  GetPCSpeaker, GetUserDefinedEventNumber, GetLastPerceived,
  GetScriptParameter(1..5) / GetScriptStringParameter (K2 dialog params),
  GetRunScriptVar.
- AssignCommand and SignalEvent are queued and run after the outermost
  script returns ("pump"), like the engine does. DelayCommand is a timer
  that resumes the STORE_STATE block with OBJECT_SELF = the caller.
  ActionDoCommand is an action-queue entry.
- ClearAllActions bumps an epoch so a step that clears its own queue (a
  DoCommand that calls ClearAllActions) does not pop the wrong action.
- Heartbeats every 6 s (module, area, creatures, doors, placeables,
  triggers). Perception: creatures with an OnNotice script notice party
  members within 20 m (sweep every 0.5 s), seen and vanished.
- Triggers fire ScriptOnEnter/ScriptOnExit for any creature (point in
  polygon, XY). Transition triggers (UTT type 1 or LinkedToModule) move the
  leader: other module -> PendingTransition, same module -> jump to the
  LinkedTo tag. Triggers the party spawns inside count as already entered,
  so arriving at a destination waypoint does not bounce back.
- Doors: see Door use below.
- Creatures: use_object/talk_to fires ScriptDialogue with
  GetListenPatternNumber() = -1 (the k_def_dialogue path calls
  BeginConversation), falling back to the creature's conversation file.
  The creature's conversation file is the fallback only when the script did
  nothing for this NPC: a bare line barked by it (`World::barks_by`, counted
  per speaker) or a conversation it started is the talk. A StartConversation
  queued for this NPC is the talk when the queue was empty (or cleared by
  ClearAllActions, which k_ai_master runs first) and the start is at its
  front: it runs on the next tick, when it reaches the front (tick.rs
  run_actions). Any other new start is dropped, so the talk runs once.
- Bare lines are barks: a conversation whose starting line has no reply
  links (`Dialog::is_one_liner`) emits `WorldEvent::BarkLine` and keeps no
  conversation state; any reply link, even a single [Continue], is a
  conversation, and so is a door's own line with text (Door use). See
  cutscenes.md.
- NotReorienting (UTC; turrets, caged animals, and many NPCs such as the
  mindrd droids and 3CFD, Ratrin): the creature does not turn to face its
  conversation partner (dialog facing, `face_each_other`, via
  `Object::turns_in_dialog`). Combat, bashing, force-power aim, scripted
  facing (SetFacing, SetFacingPoint, FaceObjectAwayFromObject) and walking
  still turn it, so NPCs that fight still turn to fight. The flag does not
  stop talk: 198 NotReorienting creature instances across the modules include
  real conversations. A turret talks its one line ("[This turret is inactive
  at the moment.]") as a conversation, since its line has a [Continue] reply.
  Survey: `cargo test --release -p k2-world --test barks survey -- --ignored
  --nocapture`.
- ActionStartConversation ownership: the non-party participant owns the
  conversation (OBJECT_SELF in its scripts). A PC told to converse with an
  NPC uses the NPC's dialog; an NPC or placeable starting one owns it. The
  prologue's "intro" is a placeable talking to itself; the PC is then the
  party leader.
- SwitchPlayerCharacter(npc) (K2): the NPC is spawned from its available-NPC
  template if needed and becomes the sole party member; the original PC is
  stashed (`World::stashed_pc`, out of the area) until
  SwitchPlayerCharacter(NPC_PLAYER = -1). 001EBO uses it to hand control to
  T3-M4.
- GetModuleName returns the module resref upper-cased ("851NIH"); scripts
  compare against that.
- Facing: NWScript uses degrees with 0 = +X counter-clockwise; k2-data
  bearings are radians for a +Y-facing model. facing = bearing + 90 deg.
- Global numbers are stored as plain ints (no byte clamping).
- Party: K2 has 12 NPC slots (NPC_ATTON = 0 ... NPC_DISCIPLE = 11, T3M4 = 8).
  AddAvailableNPCByTemplate/ByObject, AddPartyMember, RemovePartyMember,
  SpawnAvailableNPC, SetPartyLeader, IsNPCPartyMember are implemented.
  GetFirstPC / GetNextPC walk the party list in order (leader first) and
  return OBJECT_INVALID at the end. The iterator is per World, not per script.
- Effects on the target: ClearAllEffects clears the caller's applied effects;
  RemoveEffectByID drops effects of that type; RemoveEffectByExactMatch drops
  effects of the same type with the same Integer(0) and Integer(1). Effects
  with duration_type != 0 are kept in `Object::effects` (TEMPORARY 1 and
  PERMANENT 2); INSTANT (0) is dropped. Removal also drops the spell id and
  absorb pool kept for the removed handles (powers.rs), unless another object
  still carries that handle.
- ActionSwitchWeapons is a queued action (`Action::SwitchWeapons`) that swaps
  the two weapon sets when it reaches the front of the queue. It does nothing
  while a conversation is running.
- RemoveHeartbeat drops the placeable's OnHeartbeat script entry.
- Transit: SetDisableTransit(TRUE) refuses the party's module transitions
  (transition triggers and linked doors, `World::transit_blocked`). nwscript
  says only "This will disable or enable area transit". The refusal does not
  stop a script's StartNewModule, nor same-module jumps. SetDisableTransit
  resets on module entry and is saved.
- Unescapable (working interpretation, not verified against the engine):
  SetAreaUnescapable and the ARE "Unescapable" flag mean the party cannot
  leave the area by travel (the map back to the Ebon Hawk), not that
  transitions are refused. nwscript: "TRUE means you can not escape the
  area". That is why no transit checks it. Evidence: 34 of the 82 modules set
  the ARE flag (001ebo, 002ebo, 003ebo, 101per, 103per, 105per, ...),
  including the Peragus start area, whose turbolift (linked door to 105per)
  works. The travel path is not in k2-world yet: ShowGalaxyMap only emits a
  GUI event, and the presentation layer does not show a galaxy map. When that
  path is added it should check `World::area_unescapable`. A web search found
  only player reports that galaxy-map travel is story-gated, nothing on the
  flag itself. rakata's ARE page (formats/gff/are.html) lists Unescapable among
  fields nobody has examined, so the flag's engine effect is unconfirmed.
  GetAreaUnescapable returns the override when a script set one, else the ARE
  flag. SetAreaUnescapable sets `World::area_unescapable_override` (Option<bool>),
  reset on module entry and saved; saves from before the override load a set
  bool as an override of TRUE.
- Influence (influence.rs): every NPC starts at 50, clamped to 0..100. A
  change prints "Influence Gained/Lost: <name>" (TLK 48607 / 48608); an
  unchanged value prints nothing. Values survive save and load. Not
  implemented: nwscript.nss says GetInfluence returns 0 for an NPC that is not
  an available party member, returns -1 for an ambivalent party member, and
  that SetInfluence / ModifyInfluence do nothing for NPCs that are not
  available. The store keeps values for any NPC instead, because 301nar
  a_setup_cs sets influence before its NPCs are checked for availability (not
  checked). A web search gave the 50 start and 0..100 range from player
  guides, not the -1 rule. Unverified either way. The conversation gates
  (c_on_ebon: NPC 2 above 75, NPC 1 above 85) read GetInfluence, so the 50
  start keeps those dialogs closed until the player raises the values.
- Default PC until character generation exists: appearance.2da row
  labelled P_MAL_A_MED_01 (looked up by label), named "Exile".
- Combat: see combat.md (rounds, attack rolls, factions from repute.2da,
  AI driven by k_ai_master).
- Not simulated, explicit defaults: talents, force powers, skills
  (rank 0), saving throws (d20 + 2 vs DC), stealth, the swoop/turret
  minigames (SWMG_* fall through to the unimplemented histogram), pazaak
  (the end script runs as a win).
- Movement uses walkmesh paths (nav.rs, see walkmesh.md): every scripted
  move (MoveTo, MoveAway, combat approach, party follow) goes through
  `move_towards`, which steers at the next waypoint of a cached path.
  Repath when the destination moves > 1 m, when blocked, or when the mover
  drifts > 2 m off its path (at most every 0.25 s). No path: walk straight
  for 2 s, then stand still so the action's stuck handling ends it (MoveTo
  after 1.5 s stuck; combat fires OnBlocked after 4 s). Forced moves still
  snap to the destination. Engine-handle
  reclamation (the effect/event/location table only grows), animation
  lengths (fire-and-forget animations assume 1.5 s).
- Sitters (Patron{Sitter} in 503ond, {Sitting} in 207tel, Traveler{Sit} in
  201tel) queue one ActionPlayAnimation 36/37/38 (looping, duration -1) from
  their spawn script. Under k_bar_spawn they do not move. The k_bar_spawnp
  variants have a conditional move to a waypoint first; it was not taken in
  these modules. tests/sitting.rs checks one sit each in 30 s, no move, no turn.
  The client holds the sit pose (animation.md, "Sit-downs").

## Door use (lib.rs use_object)

Each use of a door takes the first step that applies (tests/door_talk.rs):

1. Every use fires OnClick first.
2. Locked: OnFailToOpen fires and the door's conversation talks (sealeddoor,
   turb101), unless OnFailToOpen already queued a start on the door; that
   start gives the line on the next tick. The door stays shut.
3. Linked to a module (LinkedToModule): the trap goes off, then the transit is
   requested. LinkedTo is the destination waypoint tag (101PER turbolift ->
   105per "From_101PER"). A refused transit (SetDisableTransit, see Transit)
   does nothing: no OnFailToOpen and no message, since nwscript,
   FeedBackText.2da and dialog.tlk show neither.
4. Static (Static 1: the cantina door in 222tel, blastdr in 153har, sealdoor in
   004ebo): never opens or closes on use. Its conversation is its answer when a
   start passes; otherwise nothing happens.
5. Open: closes (OnClosed) with no line.
6. Closed and unlocked with a conversation: the conversation talks when a start
   entry passes, and the door stays shut. When no start passes, the door opens.
   (talk_on_use also returns false while another conversation runs; the door
   then opens.)
7. Otherwise the trap goes off and the door opens (OnOpen).

Exception to step 6 (heuristic, no source in the install): a door that was
locked when its area loaded and is unlocked now skips step 6 and opens on use
with no line, even with a conversation. It does not matter whether a script or
a player (try_unlock) cleared the lock; the rule reads only the lock history.
DoorState.locked_at_load (Option<bool>) is set in spawn.rs from the template's
Locked field and saved with the door. A save from before the field takes the
template's value on load (save.rs World::load), so such a door loads as locked
at load. Known limit: a door locked at load that the player unlocked and then
closed opens on use rather than talking.

Why: PrisonRoomDr (101per, dialog prsndoor) has two starting links: E1 with
Active c_talkedto (Kreia's line), then E0 with no Active and no Active2
("magnetically sealed"). E0 always passes, so once local bool 10 on the door is
set, every use plays E0 and the door never opens. Nothing in prsndoor reads the
lock or local 30. Opening every closed, unlocked door on use was tried and
rejected: it also opens door_204sealed (204tel, template door_203sealed.utd,
dialog 204dor, an unconditional "virtually impregnable" line), the 301nar
TwinDoor (dialog 301twin, start gated by c_global_eq) and the Telos 3A2, 3B2 and
3C2 doors (232tel, dialog doorm, "cannot be opened from here"). The lock history
is the only difference found between the prison door and those.

Message boxes (`ConversationView.boxed`, dialog.rs is_message_box;
tests/door_box.rs):
- A door's own line is a box when it has text, its speaker is empty or the door
  itself, and no reply is a real choice (every reply empty, the "[Continue]"
  placeholder). The client shows it as a modal box with text and OK (gui.md).
  A line by a named speaker stays a conversation, whatever its replies.
- A door's bare line with text is a conversation, not a bark
  (start_conversation_at does not take the bark path for it), so the talkall
  bark count drops (see Harness results).
- A box does not hold a control walk (`walk_held`; tests/cutscene_hold.rs).
- Examples: 102PERDoor (dialog emrhatch) has real choices and stays a
  conversation; doorm (TelosDoor3A2) and meddoor (MedClosetDoor) are boxes;
  prsndoor E0 is a box; prsndoor E2, a door line with no text, is not a box and
  the client moves on by itself.

Unlocks (`try_unlock`, craft.rs; Security skill or a key; tests/door_talk.rs):
the lock is cleared, OnUnlock fires, and the object is used at once with no
line. A door that is not linked fires OnClick; a static door talks and never
opens; any other door opens (trap, then OnOpen) unless it is already open. A
linked door goes through use_object (its OnClick, then the transit). A container
opens. A failed check fires OnFailToOpen and the lock stays.

Containers: a container with a conversation talks on use (talk_on_use), then
opens (cont_sensor_drds, plscylsp, kre_body; secspk is unconditional, so the
container must still open). The client holds its window until the conversation
ends (gui.md). The placeable arm already did this (OnUsed, then the
conversation).

Other door dialogs (read with the tests; not rules):
- GuardRoomDr (101per, dialog grdrmdr): a Kreia scene gated by c_talkedto, which
  is true while local bool 10 (the talked flag, UT_GetTalkedToBooleanFlag) is
  unset. Nothing on grdrmdr or its door sets that flag, so the world talks on
  every use; door_talk.rs sets it by hand to see the door open. The only writer
  found is a_talktrue (SetLocalBoolean on OBJECT_SELF, value from its caller),
  called by 101atton E56. No 101per script opens GuardRoomDr or sets local 10 on
  it (the only script naming it is a_kreiaspch1), so the install gives no path
  that opens it by use. The grdrmdr reply scripts grant Precognition
  (a_giveforcepreco) and set the Kreia telepath global (a_setkretel), both on
  the first talk. GuardRoomDr is cosmetic for progression: a diagnostic walk from
  the speech point (WP_player_speech) to To_102PER routes only through the
  emergency hatch (102PERDoor), so GuardRoomDr can stay shut.
- 102PERDoor: of the 101per scripts and dialogs searched, only a_door102per
  (101atton node E41) opens it; the install's other scripts were not searched.
  The OnEnter of the triggers a_door105per and a_door103per do the opposite:
  they close and lock 102PERDoor, then open their own side hatch
  (Hatch_105PERDoor, Hatch_103PERDoor), as the trace of the 105 trigger shows.
- 301nar TwinDoor (door_narshad008, dialog 301twin): its start is gated by
  c_global_eq, and its first line is a director note.

## 101per prison door (PrisonRoomDr)

Playthrough order (tests/prison_door.rs):
- The door starts locked. A use fires OnFailToOpen, says prsndoor's line, and
  the door stays shut.
- Adm_Console's switch (admlog E3, "SECURITY OVERRIDE ACTIVATED") runs
  a_unlock_prison. E3 is reached from R2 under E5, after R4 "Look for the
  control override switch" [c_local_notset[30]], and from E1 and E2
  ("Awareness") with no condition in the dialog.
- a_unlock_prison runs k_door_heart about 0.8 s later (the switch's delayed
  run), and again on the door's 6 s heartbeat. k_door_heart clears the lock once
  no g_assassindrd01 mining droid is hostile to the PC, and sets local 30 on the
  door. The switch itself does not open the door.
- Hostility (k_door_heart and k_droids_safe): GetReputation (combat.rs
  reputation) ignores death, so a killed droid stays hostile until its corpse is
  destroyed CORPSE_SECONDS (12 s) after death (combat.rs kill). The corpse is
  kept only if the template is Lootable or DisableFade, or a script calls
  SetIsDestroyable(FALSE); none of the 101per droid templates does either. A
  destroyed object is invisible to get, iter and GetObjectByTag (lib.rs
  by_tag), so once every droid is dead and gone the tag list is empty and the
  lock clears. A droid switched to friend (a_set_friend,
  ChangeToStandardFaction 2) is not hostile.
- Walking into the KreiaPrisonWarn trigger (its script a_kreiaspch2) starts
  prsndoor: E1 is Kreia's line (a_setclaireffect), E2 the door's line
  (a_setkretel, which sets 101PER_Kreia_Telepath to 1), E3 Kreia's last line.
  The door stays shut.
- Use: the door was locked at load and is unlocked now, so it opens with no line
  (the exception above). OnOpen runs k_attontlk, which walks Atton to
  WP_ATTONCAGE and starts 101atton. Atton does not join the party at this point.

Data quirks (not changed):
- admlog R4 is gated by c_local_notset[30], with the console (the dialog owner)
  as OBJECT_SELF. k_door_heart sets local 30 on the door, not the console, so
  nothing hides R4 once the switch has been thrown.
- The 105PER_T3_End global is set only in 106per and 201tel, so on a new game
  a_hatchopen's 101atton route is not used; the east route via the console works.
- secter (the 101per security desk, SecTer): its CAMERA CONTROLS nodes and the
  EMERGENCY HATCH: SEALED nodes run no scripts (k2probe dlg secter 101per). The
  camera views were not checked on screen, and there are no pictures of them.

## Harness results (Aspyr 1.0.2)

- `k2world all --seconds 60`: 82 modules load and run 60 s at 30 Hz with
  scripted conversations auto-answered: 0 panics, 0 VM errors, 43
  conversations started by module scripts, 7 transitions, 0 unimplemented
  routine calls (2026-10-09, with this run's routine changes; the earlier
  figures were 21 and 5, and the change in counts is not attributed).
- Ebon Hawk companions (tests/ebo_companions.rs): entering 003EBO with Atton
  and Bao-Dur available runs the Atton scenes (FIRST ATTON at 1 s, SECOND
  ATTON at 2.5 s, SPAWNING PARTY at 4.9 s) and starts the Atton dialogs. The
  party does not grow in that run: companions join through the party
  selection GUI, which the headless run does not drive. A plain `k2world run
  003ebo` has no roster, so k_003ebo_enter prints "NPC could not spawn" for
  each NPC slot (DebugText; the client does not show it). A plain run of
  001ebo stops at the controls step, which waits for player input.
- Stuck-queue soak (tests/first_modules.rs, 60 s per early module): the only
  non-walking entries that stay at the front for 30 s or more are a guard's
  repeated waits and a DrdProt's repeated 3 s animation (both are scripts that
  re-queue the action, so they are not stuck), and Attack on Creature 196 in
  202tel (a 4 HP target 6.2 m away, in combat, for 30 s without a kill or a
  move). The attack case is for the combat owner. Run with K2_STUCK_DEBUG=1
  and --nocapture.
- `k2world talkall`: every object with a conversation in every module is
  used and its dialog walked with reply 0 (up to 60 lines). Run of 2026-10-09
  (talkall-why.txt, before the door bare-line rule in Door use, which moves
  door lines from barks to conversations; the next run shows fewer barks):
  1209 conversations (15 of them on the retry), 89 queued,
  68 barks, 17027 lines, 0 VM errors, 0 unimplemented routines, 9 warnings:
  the two dialogs the game does not ship (sion, 351ycs), an ActionEquipItem
  with no such item (306nar), and one missing UTC (cannock_es, 402DXN
  CreateObject). Six owners do not talk (see the known gap below).
- Routine usage: 518 of the 877 routines are used by shipped scripts. The
  per-routine status, call counts and ranked gaps are in
  docs/notes/script-coverage.md. 116 used routines are still stubs, no-ops,
  inert effects or missing.

## Party follow

Non-leader party members with an empty action queue, not in combat, not in
a conversation and not in solo mode walk after the leader when more than 3 m
away (stop at 2 m, run beyond 6 m), using the same path following. Checked in
tests/nav.rs: Kreia follows the leader 26 m across 201TEL and ends 2 m behind.

Companions get a standing follow from their heartbeat: k_ai_master calls
ClearAllActions and then ActionFollowLeader in some of its heartbeat branches.
A queue holding only follows is idle for `World::settled`, so a companion
with a heartbeat follow does not block wait_until idle. The leader follow
stops at FOLLOW_STOP = 2.0 m; that distance is a guess, nothing in nwscript
or the shipped scripts sets it. A leader follow issued to the leader itself
completes at once, so it never locks player control.

ActionFollowLeader, ActionFollowOwner and ActionForceFollowObject queue
`Action::Follow` (object.rs, `FollowTarget`). Each walks after its target and
stands within its range: the party leader (2 m, ActionFollowLeader), the
owner (`Object::owner`, fRange default 2.5, ActionFollowOwner) or the given
object (fFollowDistance default 0.0, ActionForceFollowObject). They hold still
during dialog and with no owner.

Queue rules:
- ActionFollowOwner and ActionForceFollowObject: nwscript says "once this
  action is queued, it will be the only thing this creature does until a
  ClearAllActions() is used". They never yield; only ClearAllActions removes
  them.
- ActionFollowLeader (working interpretation, not nwscript): it yields to the
  next queued action. nwscript's comment says nothing about the queue. The
  leader follow has 233 call sites, many in party AI and heartbeats, so
  blocking the queue on an inference seemed too risky.
- Dedupe: queuing a follow while one of the same target (kind or object) is
  already queued does nothing (`World::queue_follow`), so a heartbeat that
  repeats the call does not stack entries. The range of the queued entry is
  kept, so a later call with another range is ignored.

Nothing sets `Object::owner` yet (AssignPUP and SpawnAvailablePUP are stubs),
so ActionFollowOwner stands still in the game today.
k_ai_master's ActionFollowOwner(2.0) branch is the puppet AI (its string
"PUPAI - Unable to find owner. Major error"), so puppets are the only callers
and there are none in play.

ActionFollowLeaderIfNotInCombat is not declared in nwscript.nss, so no script
can call it and it is not implemented.

- Arrival with a tight range: the arrival distance is max(range, the two
  bodies' radii + 0.25 m) (nav.rs follow_towards). Creature collision holds a
  walker about 0.7 m off, so a range of 0.0 or 0.5 used to walk in place
  forever. A follower with no progress for 1.5 s stands still until the
  target is 1 m farther than where it gave up. That state (follow_stuck,
  follow_held) is a latch, serde(skip): never saved, and a new follow entry
  starts with none (queue_follow), so a clear cannot leave a hold behind.
  Walking again is a unit test (nav.rs); no route means no walk at all, so
  an unreachable target never resumes by itself.
- Refusal at queue time (queue_follow): a follow of the follower itself is
  refused, and so is any follow other than ActionFollowLeader on the party
  leader (hold-type follows would lock control until ClearAllActions). A
  leader follow of the leader itself is refused too. tick.rs also completes
  these at once if they arrive some other way, and a leader follow that
  reaches the front yields when another action is queued behind it.

Clear rules on a change of leader:
- The player's switches (portrait, Tab: set_party_leader and
  cycle_party_leader) empty the new leader's queue, as they did before
  follows existed.
- Every other change removes the new leader's follow entries of any kind
  (drop_follows) and keeps the rest of its queue: a fallen leader handed over
  in combat (party_member_down), a load that hands over a fallen leader
  (settle_loaded_party), SwitchPlayerCharacter to an NPC and back, and the
  SetPartyLeader script routine. A full clear there would drop an
  ActionDoCommand or a conversation resume and could leave a conversation
  paused. Every kind goes, not only Follow(Leader), because a hold on the new
  leader would lock control.

settled() (wait_until idle, control.rs) is the check that nothing scripted is
still to come. It needs player control (player_controlled), no queued script
event, no module change, and no pending Resume timer, whatever its caller.
The one exception is an area loop (`World::is_ambient`): a resume whose caller
is the area and whose script is already known to loop on this visit. The loop
set is `looping_scripts`, a set of (area, script) pairs, not saved. Each turn
of an area resume records whether the script re-armed (tick.rs run_timers,
`record_area_turn`, keyed by the current area), so a script that stops
looping stops counting at once, and entering a module clears the set. A
one-shot DelayCommand from any area script still blocks. Caveat: a finite
chain of an area script's own delays reads as a loop until its last turn, so
it is exempt while the chain runs. The queue check is the only party-only
part: the leader and the party members may hold Follow entries and nothing
else.
Tests: tests/script_routines.rs (follow_*, repeated_follow_calls_do_not_stack,
a_tight_follow_range_settles_instead_of_walking_in_place, the refusal and
clear-rule tests named after each path), nav.rs (held_follower), lib.rs
(area_loop_tests), tests/save_evolution.rs (follow_latches_*).

## Creature collision (crowd.rs)

- Radii: appearance.2da `perspace` is personal space and is used for
  creature-creature collision; `hitradius` stays the melee reach radius.
  `creperspace` (slightly larger) is unused for now. Typical humans 0.35 m,
  mining droid Mk I 0.9 m, rancor 3.05 m (clamped to 1.5 m). Exposed as
  `World::personal_space` (also behind GetObjectPersonalSpace semantics).
- Steps never walk into a creature: `move_towards` and `World::player_move`
  run every step through `block_creatures`, which pushes the step back out
  of other living creatures' personal space (sliding on the walkmesh).
  Overlap that already existed may persist but never deepens. The kotor2
  player controller calls `player_move` (one call site in play.rs), so the
  player slides around NPCs and never pushes them, as in KotOR.
- Separation pass each tick (after actions, combat, follow): overlapping
  pairs are pushed apart with move_slide (never off the mesh), 3 passes.
  Weights: moving 1.0, idle 0.35, the player-driven leader 0. Only pairs
  where one creature is "active" (moved, was moved, or fought within 3 s)
  are touched, so authored placements are left alone; first sighting counts
  as settled. Nothing is separated during conversations (cutscene marks).
- Local avoidance: movers bend their step away from creatures ahead within
  radii + 0.8 m; held up for over 1 s they commit to a stronger side-step.
  Paths themselves ignore creatures (blocked movers repath via nav.rs).
- Melee ring slots: a melee attacker approaching a target that others are
  already attacking takes the free angle on a ring of 0.8 x reach nearest to
  where it stands (`melee_slot`); ranged attackers and lone attackers go
  straight in. Slots must have ground within 1.5 m of the target's height.
- Harness: `k2world crowd MODULE [--seconds N]` picks the densest creature
  cluster, puts the player 6 m off it, makes 6 creatures converge on the
  player for 10 s and then fight (player min1hp), logging the minimum
  pairwise gap (distance minus both radii) every second and attacker angles
  at 4 s. Results: 102per worst gap -0.000 m, 701kor -0.000, 201tel -0.008,
  301nar -0.021; 602dan -0.152 for a single frame when a MoveTo timeout
  jump lands a receptionist on a crowd behind her counter (resolved next
  tick). `k2world fight 102per` is unchanged (player wins in ~10 s);
  `k2world all --seconds 60` unchanged (0 panics, 0 VM errors).

## Door transitions (doors.rs)

- A door's walkmesh becomes passable only when its opening animation has
  finished, and blocks again at the start of closing. `World::set_door_open`
  flips `DoorState::open`, emits `DoorChanged` and queues OnOpen/OnClosed at
  once (API unchanged: bash.rs, DoDoorAction, ActionOpenDoor and use_object all
  go through it); the walkmesh stays on the closed DWK while
  `DoorState::opening` (seconds left) is above 0, and `World::tick` counts it
  down (`tick_doors`) and then enables the open DWK. The nav grid reads the
  same enable flags, so a door in transition is closed for pathfinding.
- Duration is the length of the door model's `opening1`/`opening2` clip from
  the MDL (cached per model; clip length 0 uses 1.0 s). A model without an
  opening clip, a door without a walkmesh, and opens that happen while a
  module is being entered (initial OpenState, OnModLoad/OnSpawn/OnEnter
  scripts) are instant. Doors destroyed by bashing open through the same
  path, so they pass after their animation.
- Side: the opener's position relative to the door picks clip and DWK set,
  `DoorState::side` 2 when the opener is on the door's local +Y side (the
  leaf swings toward -Y, matching the DWK sets that differ, e.g. dor_lta03,
  dor_dxn04), else 1 (also for opens by the door itself, scripts and invalid
  openers). Open2 falls back to Open1 when the door has no second DWK and to
  the opening1 length when the model has no opening2. This is inferred, not
  taken from the original binary. The client plays `opening<side>`/
  `closing<side>` (falling back to the "1" clips), so visual and collider use
  the same length.
- Closing while still opening cancels the timer and blocks at once. Saves keep
  `side` and `opening` (serde defaults, old saves load); on load
  (`attach_object_walks`) a door saved mid transition is settled: open doors
  are passable immediately, closed ones closed.
- Line of sight, perception (OnNotice), GetObjectSeen/HasLineOfSight, stealth
  and attack decisions all read the same walkmesh enable flags (`can_see` ->
  `line_of_sight`), and the door stays on the closed DWK during opening, so
  enemies neither see nor shoot through a door until the animation ends and
  lose sight again as soon as closing starts. Every closed door DWK carries
  LOS-blocking faces (materials Nonwalk/Obscuring), so no door is see-through
  while closed.
- Tests: tests/doors.rs (passable only after the animation, no sight or shots through a door until open, blocked again at
  close, side selection, save mid transition, module entry settled). The
  client logs `door TAG opening at frame N (side, passable in X s)` and
  `door TAG passable at frame M`; `K2_GUI_USE=PeragusDoor1 kotor2 --module
  101per --screenshot P --frames 240` showed 1.19 s and 119 frames (1.2 s
  clip) between them.

## Encounters (encounter.rs)

GIT encounters: a polygon that spawns creatures when hostile creatures enter
it. Sources: the BioWare Aurora Encounter Format document (the nwn.wiki copy,
read as text with pdftotext: field meanings, Active, Reset, Respawns,
SpawnOption, Orientation as "radians counterclockwise from north", spawn
points in absolute area coordinates, geometry relative to the encounter
position) and rakata's UTE page
(https://synchro.codeberg.page/rakata/formats/gff/ute.html: absent-field
defaults and the ascending CR sort). Where neither says, the rules below are
guesses, marked "(guess)". NWN Lexicon notes that an exhausted encounter goes
inactive come from a community page (nwnlexicon.com returned 403; the text
was read from search snippets): NWN-derived, not checked in the game.

Survey (`k2probe encounters`, all 82 modules): 69 GIT encounters in 15
modules. Active 35, inactive 34. SpawnOption 1 (single shot) 68, 0
(continuous) 1 (402dxn g_drdassagrou001, Reset 1, ResetTime 1, Respawns 10).
PlayerOnly 49. Faction 1 for 68, faction 2 for one (506ond ls_noncombat, which
the player is not hostile to, so it never fires). Only OnEntered is used by
shipped encounters (13: 106per, 301nar, 303nar, 351nar); OnExit, OnHeartbeat,
OnExhausted and OnUserDefined are never set. CreatureList 0-5 entries with CR
1..9; 3 turrets in 231tel list none. Geometry 1-15 points. Spawn points 0-8.
506ond ls_vaklu_enc (-35.3, 43.2) has one geometry point. A one-vertex
polygon encloses no area, so it can never fire. 103per (103drdambush, CR 2
droids, rec 4) and 106per (106drdambush, OnEntered) are the early modules with
encounters.

- Defaults for absent fields (rakata): Active 1, Faction 1, ResetTime 60,
  MaxCreatures 8, RecCreatures 2, Respawns 0, SpawnOption 0, Reset 0,
  PlayerOnly 0. (The MaxCreatures and RecCreatures defaults are rakata's; their
  use here is a guess.)
- Trigger: each tick, living creatures inside the polygon (XY) that qualify.
  PlayerOnly: party members and PCs; otherwise any creature, except the
  encounter's own spawns. Dead and knocked-down (down) creatures never
  qualify, so they neither enter nor count as inside. OnEntered (GetEnteringObject) and OnExit fire for
  qualifying creatures whatever the Active flag and faction. One pass over the
  creatures per tick serves every encounter, with a bounding-box test before
  the point-in-polygon test.
- Revisit: occupancy is forgotten when the party leaves the module (like
  triggers). On a revisit every qualifying creature standing inside an
  encounter counts as already inside, so arriving does not fire OnEntered
  again (the party only, for triggers). Tested with the party leader.
- Spawn needs Active and a hostile qualifying creature inside, where hostile
  means reputation(creature faction, encounter faction) <= 10 (factions.rs).
- Budget (guess): encounter level = average class levels of the party (at
  least 1) + the encdifficulty.2da VALUE for DifficultyIndex (0, 1, 2, 5, 9
  for VeryEasy .. Impossible). A non-guaranteed creature spawns only when its
  CR is at or below the level. The format document says the count can fall
  short of RecCreatures when nothing fits, which this follows.
- Pool order: the CreatureList is sorted by ascending CR, stable (rakata:
  "bubble-sorts the spawn pool by ascending CR"; a display order in the file
  does not survive). Each spawn takes the first entry in that order that fits
  the level and is not blocked by SingleSpawn, so the cheapest creature comes
  first and the next spawn moves on once it is blocked or too strong.
- Count: RecCreatures alive at once, never more than MaxCreatures.
  GuaranteedCount (a K2 field, not in the format document) copies are owed
  per cycle: they spawn first and ignore the budget. The owed copies are reset
  when a cycle begins, so a continuous encounter refilling mid-cycle does not
  spawn them again (guess on the semantics; the per-cycle cap is the fix).
  SingleSpawn allows one alive per resref.
- Positions: spawn points (Orientation is the creature bearing, radians
  counterclockwise from north, per the format document), or the polygon
  vertices without spawn points. Both are snapped to the walkmesh once, on
  first use: the point itself, else the first ground within 0.5, 1 or 2 m in
  eight directions. A point with no walkable ground within 2 m is skipped and
  logged once. Each spawn then takes the slot furthest from the triggering
  creature first (guess: a community wiki note, not in the format document).
  New creatures get ScriptSpawn (k_def_spawn01 sets local 11 and listens;
  tests/encounters.rs checks it) and ObjectSpawned.
- Cycles: a cycle begins when a hostile creature stands inside. Single shot
  (SpawnOption 1) spawns one cycle per trigger; continuous (0) refills toward
  RecCreatures while occupied. SpawnsMax = Respawns + 1 (-1 = unlimited).
- End of a cycle (all its creatures dead). With Reset 1 and cycles left, the
  encounter goes inactive and waits ResetTime seconds, then becomes active
  again. Otherwise it is exhausted: Active is false, and OnExhausted fires
  once. This covers Reset 0 whatever the SpawnOption, since the format document
  says Reset 0 means no respawn (no re-arm for a continuous encounter with
  Reset 0; the NWN Lexicon's exhaustion note matches). A new cycle clears
  Exhausted, and OnExhausted fires again when the next cycle ends.
  `exhaustions` counts the OnExhausted firings (the dispatch, not what the
  script does; the tests check the counter).
- Scripts and the wait (guess: the format document and the Lexicon notes do
  not say what a script's setting does to a wait): SetEncounterActive (also
  World::set_encounter_active) cancels a ResetTime wait in progress, whichever
  value it sets. The wait would have switched the encounter back on at its
  end; a script's setting takes its place. Setting TRUE with the party inside
  spawns at the next tick.
- Spent encounters: SetEncounterActive(TRUE) on an encounter with no cycles
  left sets the flag (GetEncounterActive reports 1) and nothing spawns until
  SpawnsMax or SpawnsCurrent change.
- Heartbeat: OnHeartbeat every 6 s, as for creatures. OnUserDefined goes
  through SignalEvent's generic label path (not exercised by a test).
- Routines: GetEncounterActive, SetEncounterActive, Get/SetEncounterSpawnsMax,
  Get/SetEncounterSpawnsCurrent, Get/SetEncounterDifficulty,
  GetIsEncounterCreature (host.rs calls encounter_call first).
- Save: the state (active, counters, polygon, inside set, spawned list, slot
  cache, reset countdown, exhausted, exhaustion count) is the object's Detail,
  so a spent encounter stays spent. EncounterState and SpawnEntry are
  serde(default). Spawned creatures carry CreatureState.encounter. Saves from
  before encounters had state (encounter objects with Detail::None) get it back
  on load and on module re-entry: the GIT instance with the same template and
  position supplies the blueprint data, one instance per object, so the
  several ls_vaklu_enc of 506ond each get their own. state_digest covers the
  encounter state (flags, counters, spawned, inside, spawns max, owed copies).
- Tests: crates/k2-world/tests/encounters.rs. 103per: walking the leader in
  spawns the 4 droids at spawn points, staying in does not spawn a second
  cycle, the kill exhausts the ambush (Active false, the exhaustion counter
  reads 1 and stays 1), a save keeps the spent state, SingleSpawn holds one
  droid alive. 301nar redeclipse001: OnEntered runs (k_recspawn reads the
  encounter's local 30 and a global, seen with K2_CALLS=1) and the encounter
  stays inactive. 106per: with 101PER_Sion_Arrives = 1, the OnEntered
  t_spawndrd calls SetEncounterActive(TRUE, self) and 3 droids spawn; leaving
  and walking back in fires OnEntered again, but a revisit with the party
  inside does not. 504ond: the area OnEnter activates ls_cowards and
  deactivates ds_barrack_guard. 402dxn: a reset encounter respawns after
  ResetTime; a script's deactivation during the wait cancels it, a script's
  activation replaces it; a continuous encounter with Reset 0 is exhausted
  when its cycle ends; a new cycle after exhaustion clears Exhausted.
  A knocked-down creature neither enters a 506ond polygon nor counts as
  inside while it is down. 506ond ls_vaklu_enc001: a level 1 party gets its
  GuaranteedCount copies despite the CR budget (none without them). Pool order
  is tested on a synthetic blueprint (ascending CR, file order for ties). A
  pre-change save loads with its encounters rebuilt (the live objects, the
  shared-template ones of 506ond one to one, and the remembered module on
  re-entry).
- 103per entry cutscene: entering 103per queues an ACTION_DIALOGOBJECT on HK50
  (tag HK50, object 156), so the hk50 conversation starts on the first tick
  with no party movement. Its scripts then give the leader ACTION_MOVETOPOINT
  (run) steps from the first tick and leave the ambush polygon within about a
  second. Ending the conversation does not stop those steps. Tests that need
  the leader inside use 402dxn, which has no entry cutscene. Not an encounter
  fault.
- Harness: `k2world all --seconds 60` leaves the party at the entry, so no
  encounter fires (0). With `K2_VISIT_ENCOUNTERS=1` the leader steps into each
  encounter in turn every 0.5 s (the schedule cycles through the encounters
  for 60 s): 18 encounters begin a cycle and 72 creatures spawn (103per 1/4,
  151har 4/16, 152har 5/20, 154har 4/16, 504ond 1/8, 506ond 3/8;
  fired/creatures). 82 modules, 0 panics, 0 VM errors.
  A run in between gave 21 and 84 (506ond 6/20). The three extra fires came
  from the only party member being knocked down (not dead) at step 70, about
  2.3 s, in the ls_vaklu_enc001 fight (spawned creatures). A knocked-down
  creature used to count as standing for encounters, so the harness kept
  placing it and it fired ls_vaklu_enc001 at step 106 and ls_vaklu_enc and
  ls_vaklu_enc001 at step 136. Knocked-down creatures no longer qualify (see
  Trigger), and the count is back to 18 and 72. The knockdown rules are in
  party.rs and combat.rs. 506ond has no Reset, no waits and the run does not
  revisit.
- Cost: the idle encounter pass on 506ond (26 encounters, party at the entry)
  was about 40 us per tick, and is about 4.4 us after one shared creature pass
  with a bounding-box reject. Measured with
  `cargo test --release -p k2-world --test encounters idle_tick -- --ignored --nocapture`
  (the same tick with the encounter detail removed is the baseline).
- Not done: the budget, pool and count rules are guesses (above); the NWN
  Lexicon note is community text; the client was not run to see spawns.

## Crafting and locks

Workbench/lab screens, item creation and breakdown, upgrades and Security
lock picking live in craft.rs; see crafting.md.

## Known gap: first-click retries (talkall, 2026-10-09)

`k2world talkall` talks to each owner once with use_object, and where that
does not open a conversation it retries talk_to once (counted as "retried").
After the door and container fixes, 15 owners still need the retry, and 6
owners do not talk at all (run of 2026-10-09, talkall-why.txt). Four are
explained: their start conditions are false in a fresh module (plot state:
destroytu, sullust, 203b4d4, 203lorso). Two are dialogs the install does not
ship (sion for DarthSion in 154har, 351ycs for InvMira in 351nar). The 15 are documented here
rather than fixed, because the first call's cause is not pinned down.

- Doors open when clicked: TelosDoor3A1, TelosDoor3B1 and TelosDoor3C1
  (door_tel004, 006, 010) have OpenState 1 in their templates, so the first
  click closes them (the open branch of use_object) and plays no line. Their
  twins (3A2, 3B2, 3C2; OpenState 0) talk on the first click, and the closed
  doors then say "cannot be opened from here". Whether the original plays the
  line on closing is not known; this is the rule as implemented.
- Doors that are probably open before the talk (not verified): Airlock1a
  (closed and locked in its template, OnFailToOpen a_compdlg) and TO_851NIH
  (static, closed in its template). Plot scripts in the module entry may open
  them; a trace would show it.
- Creatures: GenoDisc (x6, 305nar, the developer cheat node), JediCounM
  (711kor), ond_sold (x2) and ond_door (222tel). Their start passes
  (World::explain_start) and no conversation is open. The single-owner trace
  (`k2world talk`, which settles the module the same way) succeeds on the
  first click: GetCommandable = 1, then BeginConversation = 1 (traces of
  GenoDisc and JediCounM). In talkall the first fire fails with the same data.
  The difference is the state the earlier owners and the module intro leave
  behind. The JediCounM trace shows k_ai_master events 1006 and 1007 running
  before its 1004 talk, and the entrance door queueing entrance_cs; the
  full run's order may differ. Not reproduced by the single run; a per-owner
  state dump before the first talk would show which flag differs.

Not done: changing the first-click behaviour for these owners without that
dump, since the trace above shows the scripts do talk when the state is right.

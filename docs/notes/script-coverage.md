# Script routine coverage

Which engine routines the shipped scripts call, and what k2-world does with each.

Measured by k2-script's routine scanner over chitin and all module capsules (5120 NCS resources): 518 routines are used, 402 real, 116 are gaps. Static classification of the dispatch code in crates/k2-world/src and the k2-script builtins. real means a dispatch arm exists with a non-trivial body; it was not checked line by line, so a real row can still be thin. The top gap rows were read by hand. The follow and puppet rows were re-measured with rscan (ActionFollowLeader 233 sites, 78 scripts; ActionFollowOwner 78/78; GetPUPOwner 155/78; SetInfluence 10/1; ActionEquipItem 42/18). ActionForceFollowObject has no shipped call site, so it is not in the table.

## Method

- sites: ACTION calls to the routine across all scripts. scripts: distinct NCS that call it.
- early: sum over the calling module groups of weight x scripts. Weights: Ebon Hawk (ebo) and Peragus (per) 4; Telos (tel) and 15x har 3; chitin 2; everything else 1.
  The chapter order of the other modules is an assumption (from the numeric prefixes), not checked against the game.
- status: real (implemented), stub (fixed default), noop (accepted and ignored), inert (effect value that nothing reads), partial (some of the routine's cases are handled and others are not: an effect name read somewhere in k2-world, a routine answered by several handlers that can fall through, or a stub that reads a field nothing sets), missing (not dispatched).
- owner: the area whose agent should take the gap. scripts = this run. Owners follow the lead's split: rules (combat, stats, progression), traps (traps, mines, stealth, bash), pathing (movement), hud (presentation and GUI), audio, presentation, minigames, dialog, items.

## Top 30 gaps (by early score)

| # | routine | status | returns | sites | scripts | modules | early | first | owner | note |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | EffectBeam | inert | Engine(0) | 220 | 116 | 37 | 184.0 | 003ebo | presentation | visual beam only, applied as a value; no gameplay expected |
| 2 | ShipBuild | stub | Int | 103 | 88 | 35 | 145.0 | 003ebo | - | returns 1 = shipping build (nwscript 761); correct default |
| 3 | EffectParalyze | partial | Engine(0) | 83 | 81 | 32 | 130.0 | 003ebo | rules | debilitates via powers.rs DEBILITATING list; no other effect behaviour |
| 4 | EffectSleep | partial | Engine(0) | 79 | 79 | 32 | 128.0 | 003ebo | rules | debilitates via powers.rs DEBILITATING list |
| 5 | GetIsPuppet | stub | Int | 155 | 78 | 32 | 125.0 | 003ebo | scripts | false for everything; puppets (PUP table) not modelled; false is right unless puppets exist |
| 6 | GetPUPOwner | partial | Object | 155 | 78 | 32 | 125.0 | 003ebo | scripts | invalid unless Object::owner is a party member (nwscript 841); nothing sets Object::owner yet (AssignPUP, SpawnAvailablePUP are stubs) |
| 7 | GetHealTarget | stub | Object | 77 | 77 | 32 | 123.0 | 003ebo | rules | invalid; healer AI target not stored (nwscript 814) |
| 8 | GetPlayerRestrictMode | stub | Int | 185 | 77 | 32 | 123.0 | 003ebo | hud | always FALSE |
| 9 | GetUserActionsPending | stub | Int | 91 | 77 | 32 | 123.0 | 003ebo | rules | always FALSE (combat queue not modelled) |
| 10 | SetHealTarget | noop | Void | 308 | 77 | 32 | 123.0 | 003ebo | rules | accepted and dropped; healer AI target not stored (nwscript 813) |
| 11 | RevealMap | noop | Void | 38 | 38 | 29 | 103.0 | 001ebo | hud | no-op; map fog of war not modelled |
| 12 | GetLoadFromSaveGame | stub | Int | 43 | 43 | 41 | 88.0 | 001ebo | scripts | always FALSE; correct here because World::load runs no scripts |
| 13 | SetCreatureAILevel | noop | Void | 78 | 38 | 19 | 82.0 | 003ebo | rules | no-op; creature AI priority not modelled |
| 14 | ShowTutorialWindow | noop | Void | 21 | 19 | 5 | 76.0 | 001ebo | hud | no-op; tutorial popups (tutorial.2da) not shown |
| 15 | DisableVideoEffect | noop | Void | 23 | 23 | 12 | 72.0 | 001ebo | presentation | no-op; frame-buffer video effects not applied |
| 16 | EnableVideoEffect | noop | Void | 22 | 22 | 17 | 70.0 | 001ebo | presentation | no-op; frame-buffer video effects not applied |
| 17 | SetOrientOnClick | noop | Void | 46 | 43 | 12 | 63.0 | 003ebo | hud | no-op; creatures do not turn to the player on click |
| 18 | SetDialogPlaceableCamera | noop | Void | 65 | 26 | 16 | 51.0 | 003ebo | hud | no-op; dialog camera cut to placeable not implemented |
| 19 | GetIsXBox | stub | Int | 18 | 16 | 11 | 46.0 | 001ebo | - | FALSE (PC build); correct |
| 20 | EffectPoison | inert | Engine(0) | 34 | 22 | 14 | 45.0 | 101per | rules | inert: no poison damage over time |
| 21 | SurrenderToEnemies | noop | Void | 52 | 30 | 14 | 45.0 | 003ebo | rules | no-op; combat surrender not modelled |
| 22 | AdjustCreatureAttributes | noop | Void | 28 | 16 | 3 | 37.0 | 003ebo | rules | no-op; ability adjust not applied (stat formulas) |
| 23 | CreatureFlourishWeapon | noop | Void | 33 | 22 | 16 | 36.0 | 001ebo | presentation | no-op; cosmetic flourish |
| 24 | SetForfeitConditions | noop | Void | 23 | 15 | 4 | 35.0 | 003ebo | minigames | no-op; minigame forfeit rules |
| 25 | GetAttemptedSpellTarget | stub | Object | 32 | 15 | 1 | 30.0 | chitin | rules | invalid; combat target not stored |
| 26 | EffectDisguise | inert | Engine(0) | 11 | 11 | 8 | 29.0 | 001ebo | rules | inert |
| 27 | ResetDialogState | noop | Void | 16 | 14 | 1 | 28.0 | chitin | dialog | no-op; engine dialog global state not kept |
| 28 | SetLightsaberPowered | noop | Void | 47 | 19 | 13 | 25.0 | 262tel | presentation | no-op; blade on/off override not shown |
| 29 | PlayMovieQueue | noop | Void | 14 | 10 | 6 | 23.0 | 003ebo | hud | no-op; QueueMovie/PlayMovie route through WorldEvent::Movie instead |
| 30 | SetFakeCombatState | noop | Void | 27 | 12 | 9 | 23.0 | 153har | presentation | no-op; fake combat flag not kept |

## Changed in this run

- GetNextPC: walks party members in order (was invalid after the first).
- ClearAllEffects, RemoveEffectByID, RemoveEffectByExactMatch: remove applied effects from the target (were no-ops).
- ActionSwitchWeapons: queued action that swaps the two weapon sets when it reaches the front; skipped in dialog (was a no-op).
- RemoveHeartbeat: drops the placeable OnHeartbeat script (was a no-op).
- SetDisableTransit: refuses the party's transitions (transition triggers and linked doors) until cleared; reset on module entry and saved (was a no-op). StartNewModule is not refused.
- SetAreaUnescapable: sets the override that GetAreaUnescapable returns (the ARE flag otherwise). It does not refuse transit: no evidence the engine does, and 34 of 82 modules set the flag.
- ActionFollowLeader, ActionFollowOwner: queue a follow that walks after the leader or Object::owner (were no-ops). ActionFollowOwner holds the queue until ClearAllActions (nwscript 843); ActionFollowLeader yields to the next queued action (working interpretation). A repeat of the same follow is not queued again. A tight range (0.0 or 0.5) arrives at the two bodies' radii plus 0.25 m, and a follower with no progress for 1.5 s stops trying.
- ActionForceFollowObject: queues a follow of the given object until ClearAllActions (nwscript 167). No shipped script calls it.
- GetPUPOwner: returns Object::owner when it is a party member (nwscript 841). Nothing sets Object::owner yet, so it is still invalid in play.
- RemoveEffect, ClearAllEffects, RemoveEffectByID, RemoveEffectByExactMatch, and effects that expire or are fully absorbed drop their spell id and absorb pool entries, unless another copy of the same handle is still applied.
- ActionEquipItem, ActionUnequipItem, SetInfluence: dead copies in host.rs removed. inventory.rs equip() / unequip() and influence.rs handle them; status unchanged.
- GIT encounters (encounter.rs): SetEncounterActive, GetEncounterActive, Get/SetEncounterSpawnsMax, Get/SetEncounterSpawnsCurrent, Get/SetEncounterDifficulty, GetIsEncounterCreature are real (were a no-op and missing). Encounters spawn creatures when creatures hostile to the encounter's faction enter them (party members only for PlayerOnly encounters); the GetEncounter* and SetEncounterSpawns* routines are not called by the shipped scripts.

## Every gap, ranked

| # | routine | status | returns | sites | scripts | modules | early | first | owner | note |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | EffectBeam | inert | Engine(0) | 220 | 116 | 37 | 184.0 | 003ebo | presentation | visual beam only, applied as a value; no gameplay expected |
| 2 | ShipBuild | stub | Int | 103 | 88 | 35 | 145.0 | 003ebo | - | returns 1 = shipping build (nwscript 761); correct default |
| 3 | EffectParalyze | partial | Engine(0) | 83 | 81 | 32 | 130.0 | 003ebo | rules | debilitates via powers.rs DEBILITATING list; no other effect behaviour |
| 4 | EffectSleep | partial | Engine(0) | 79 | 79 | 32 | 128.0 | 003ebo | rules | debilitates via powers.rs DEBILITATING list |
| 5 | GetIsPuppet | stub | Int | 155 | 78 | 32 | 125.0 | 003ebo | scripts | false for everything; puppets (PUP table) not modelled; false is right unless puppets exist |
| 6 | GetPUPOwner | partial | Object | 155 | 78 | 32 | 125.0 | 003ebo | scripts | invalid unless Object::owner is a party member (nwscript 841); nothing sets Object::owner yet (AssignPUP, SpawnAvailablePUP are stubs) |
| 7 | GetHealTarget | stub | Object | 77 | 77 | 32 | 123.0 | 003ebo | rules | invalid; healer AI target not stored (nwscript 814) |
| 8 | GetPlayerRestrictMode | stub | Int | 185 | 77 | 32 | 123.0 | 003ebo | hud | always FALSE |
| 9 | GetUserActionsPending | stub | Int | 91 | 77 | 32 | 123.0 | 003ebo | rules | always FALSE (combat queue not modelled) |
| 10 | SetHealTarget | noop | Void | 308 | 77 | 32 | 123.0 | 003ebo | rules | accepted and dropped; healer AI target not stored (nwscript 813) |
| 11 | RevealMap | noop | Void | 38 | 38 | 29 | 103.0 | 001ebo | hud | no-op; map fog of war not modelled |
| 12 | GetLoadFromSaveGame | stub | Int | 43 | 43 | 41 | 88.0 | 001ebo | scripts | always FALSE; correct here because World::load runs no scripts |
| 13 | SetCreatureAILevel | noop | Void | 78 | 38 | 19 | 82.0 | 003ebo | rules | no-op; creature AI priority not modelled |
| 14 | ShowTutorialWindow | noop | Void | 21 | 19 | 5 | 76.0 | 001ebo | hud | no-op; tutorial popups (tutorial.2da) not shown |
| 15 | DisableVideoEffect | noop | Void | 23 | 23 | 12 | 72.0 | 001ebo | presentation | no-op; frame-buffer video effects not applied |
| 16 | EnableVideoEffect | noop | Void | 22 | 22 | 17 | 70.0 | 001ebo | presentation | no-op; frame-buffer video effects not applied |
| 17 | SetOrientOnClick | noop | Void | 46 | 43 | 12 | 63.0 | 003ebo | hud | no-op; creatures do not turn to the player on click |
| 18 | SetDialogPlaceableCamera | noop | Void | 65 | 26 | 16 | 51.0 | 003ebo | hud | no-op; dialog camera cut to placeable not implemented |
| 19 | GetIsXBox | stub | Int | 18 | 16 | 11 | 46.0 | 001ebo | - | FALSE (PC build); correct |
| 20 | EffectPoison | inert | Engine(0) | 34 | 22 | 14 | 45.0 | 101per | rules | inert: no poison damage over time |
| 21 | SurrenderToEnemies | noop | Void | 52 | 30 | 14 | 45.0 | 003ebo | rules | no-op; combat surrender not modelled |
| 22 | AdjustCreatureAttributes | noop | Void | 28 | 16 | 3 | 37.0 | 003ebo | rules | no-op; ability adjust not applied (stat formulas) |
| 23 | CreatureFlourishWeapon | noop | Void | 33 | 22 | 16 | 36.0 | 001ebo | presentation | no-op; cosmetic flourish |
| 24 | SetForfeitConditions | noop | Void | 23 | 15 | 4 | 35.0 | 003ebo | minigames | no-op; minigame forfeit rules |
| 25 | GetAttemptedSpellTarget | stub | Object | 32 | 15 | 1 | 30.0 | chitin | rules | invalid; combat target not stored |
| 26 | EffectDisguise | inert | Engine(0) | 11 | 11 | 8 | 29.0 | 001ebo | rules | inert |
| 27 | ResetDialogState | noop | Void | 16 | 14 | 1 | 28.0 | chitin | dialog | no-op; engine dialog global state not kept |
| 28 | SetLightsaberPowered | noop | Void | 47 | 19 | 13 | 25.0 | 262tel | presentation | no-op; blade on/off override not shown |
| 29 | PlayMovieQueue | noop | Void | 14 | 10 | 6 | 23.0 | 003ebo | hud | no-op; QueueMovie/PlayMovie route through WorldEvent::Movie instead |
| 30 | SetFakeCombatState | noop | Void | 27 | 12 | 9 | 23.0 | 153har | presentation | no-op; fake combat flag not kept |
| 31 | SetLockHeadFollowInDialog | noop | Void | 15 | 10 | 7 | 22.0 | 103per | presentation | no-op; head follow lock in dialog |
| 32 | EffectChoke | partial | Engine(0) | 25 | 13 | 10 | 21.0 | 101per | rules | debilitates via powers.rs DEBILITATING list |
| 33 | EffectForcePushTargeted | partial | Engine(0) | 17 | 13 | 10 | 18.0 | 101per | rules | push effect; movement part not modelled |
| 34 | EffectStunned | partial | Engine(0) | 13 | 8 | 5 | 18.0 | 102per | rules | debilitates via powers.rs DEBILITATING list |
| 35 | EnableRendering | noop | Void | 9 | 9 | 9 | 16.0 | 001ebo | presentation | no-op; object rendering toggle not implemented |
| 36 | SetForceAlwaysUpdate | noop | Void | 27 | 13 | 7 | 16.0 | 222tel | scripts (open) | no-op; far-object update override not implemented |
| 37 | SpawnAvailablePUP | stub | Object | 6 | 6 | 6 | 15.0 | 003ebo | scripts | invalid; puppets not modelled |
| 38 | StopRumblePattern | noop | Int | 4 | 4 | 2 | 15.0 | 103per | - | no-op; controller rumble not implemented (fine) |
| 39 | EffectCutSceneHorrified | inert | Engine(0) | 7 | 5 | 5 | 14.0 | 105per | presentation | inert; cutscene visual |
| 40 | MusicBackgroundPlay | noop | Void | 7 | 6 | 4 | 14.0 | 101per | audio | no-op; scripted background music not routed to audio |
| 41 | EffectCutSceneParalyze | inert | Engine(0) | 5 | 5 | 5 | 13.0 | 101per | presentation | inert; cutscene visual |
| 42 | MusicBackgroundStop | noop | Void | 5 | 5 | 3 | 13.0 | 101per | audio | no-op; scripted background music not routed to audio |
| 43 | PlayRumblePattern | stub | Int | 4 | 4 | 4 | 13.0 | 003ebo | - | returns 0; no controller rumble (fine) |
| 44 | SetPlayerRestrictMode | noop | Void | 7 | 7 | 5 | 13.0 | 104per | hud | no-op; no input gate |
| 45 | EffectAssuredHit | inert | Engine(0) | 7 | 7 | 4 | 11.0 | 202tel | rules | inert; combat hit guarantee not applied |
| 46 | ResetCreatureAILevel | noop | Void | 41 | 5 | 5 | 11.0 | 203tel | rules | no-op; creature AI priority not modelled |
| 47 | EffectDroidStun | partial | Engine(0) | 9 | 4 | 2 | 10.0 | 103per | rules | debilitates via powers.rs DEBILITATING list |
| 48 | EffectHorrified | partial | Engine(0) | 7 | 4 | 2 | 10.0 | 107per | rules | debilitates via powers.rs DEBILITATING list |
| 49 | SetInputClass | noop | Void | 4 | 4 | 4 | 9.0 | 107per | hud | no-op; input class (dialog, minigame, GUI) not routed |
| 50 | DisableHealthRegen | noop | Void | 2 | 2 | 1 | 8.0 | 001ebo | rules | no-op; innate regen toggle not implemented |
| 51 | EffectBlind | inert | Engine(0) | 2 | 2 | 2 | 6.0 | 102per | rules | inert |
| 52 | GetIsPoisoned | stub | Int | 10 | 3 | 1 | 6.0 | chitin | rules | FALSE; poison not modelled |
| 53 | MusicBattlePlay | noop | Void | 2 | 2 | 2 | 6.0 | 001ebo | audio | no-op; battle music not routed to audio |
| 54 | ActionUseSkill | partial | Void | 8 | 2 | 2 | 5.0 | 101per | - | security, stealth and demolitions (with a trap) are answered by craft, stealth and traps; other skills accepted and ignored |
| 55 | AddAvailablePUPByObject | stub | Int | 2 | 2 | 2 | 5.0 | 231tel | scripts | TRUE; puppets not modelled |
| 56 | AssignPUP | stub | Int | 2 | 2 | 2 | 5.0 | 231tel | scripts | returns 0; puppets not modelled |
| 57 | ModifyWillSavingThrowBase | noop | Void | 3 | 3 | 2 | 5.0 | chitin | rules | no-op; saving throw base not adjusted |
| 58 | SoundObjectGetPitchVariance | missing | Float | 3 | 3 | 3 | 5.0 | 211tel | presentation | missing: not dispatched, returns 0.0 |
| 59 | AdjustCreatureSkills | noop | Void | 3 | 2 | 1 | 4.0 | chitin | rules | no-op; skill adjust not applied |
| 60 | DisableMap | noop | Void | 4 | 4 | 1 | 4.0 | 305nar | hud | no-op; map disable not modelled |
| 61 | EffectForcePushed | partial | Engine(0) | 7 | 2 | 1 | 4.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 62 | EffectForceShield | inert | Engine(0) | 2 | 2 | 1 | 4.0 | chitin | rules | inert; force shield absorb not applied |
| 63 | GetGameDifficulty | stub | Int | 2 | 1 | 1 | 4.0 | 001ebo | - | returns 1 (normal) fixed |
| 64 | GetIsLinkImmune | stub | Int | 6 | 2 | 1 | 4.0 | chitin | rules | FALSE; immunity links not modelled |
| 65 | GetItemHasItemProperty | stub | Int | 1 | 1 | 1 | 4.0 | 101per | items | FALSE; item property check not modelled |
| 66 | GetLastWeaponUsed | stub | Object | 1 | 1 | 1 | 4.0 | 101per | rules | invalid; last weapon not tracked |
| 67 | ModifyReflexSavingThrowBase | noop | Void | 2 | 2 | 1 | 4.0 | chitin | rules | no-op |
| 68 | RebuildPartyTable | noop | Void | 1 | 1 | 1 | 4.0 | 003ebo | scripts | no-op; party is a plain list here, so nothing to rebuild |
| 69 | EffectCutSceneStunned | inert | Engine(0) | 3 | 3 | 3 | 3.0 | 506ond | presentation | inert; cutscene visual |
| 70 | ActionRandomWalk | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 71 | CancelPostDialogCharacterSwitch | noop | Void | 1 | 1 | 1 | 2.0 | chitin | dialog | no-op; post-dialog switch cancel |
| 72 | DisplayFeedBackText | noop | Void | 9 | 1 | 1 | 2.0 | chitin | hud | no-op; feedback text not shown |
| 73 | EffectBodyFuel | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 74 | EffectCrush | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 75 | EffectDamageImmunityIncrease | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert; damage immunity not applied |
| 76 | EffectDroidScramble | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 77 | EffectEntangle | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 78 | EffectFactionModifier | inert | Engine(0) | 3 | 1 | 1 | 2.0 | chitin | rules | inert; faction change via effect not applied |
| 79 | EffectForceBody | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 80 | EffectForceFizzle | inert | Engine(0) | 19 | 1 | 1 | 2.0 | chitin | presentation | inert; visual on force power fizzle |
| 81 | EffectForceResisted | inert | Engine(0) | 6 | 1 | 1 | 2.0 | chitin | rules | inert |
| 82 | EffectForceSight | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 83 | EffectFury | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 84 | EffectImmunity | inert | Engine(0) | 7 | 1 | 1 | 2.0 | chitin | rules | inert; immunity not applied |
| 85 | EffectLightsaberThrow | inert | Engine(0) | 4 | 1 | 1 | 2.0 | chitin | rules | inert |
| 86 | EffectMindTrick | partial | Engine(0) | 2 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 87 | EffectPsychicStatic | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 88 | EffectRegenerate | inert | Engine(0) | 2 | 1 | 1 | 2.0 | chitin | rules | inert; regeneration not applied |
| 89 | EffectSkillIncrease | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 90 | EffectTrueSeeing | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 91 | EffectVPRegenModifier | inert | Engine(0) | 3 | 1 | 1 | 2.0 | chitin | rules | inert |
| 92 | EffectWhirlWind | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | - | effect name read by k2-world (see powers.rs) |
| 93 | GetAttemptedMovementTarget | stub | Object | 1 | 1 | 1 | 2.0 | chitin | pathing | invalid |
| 94 | GetBlockingCreature | stub | Object | 1 | 1 | 1 | 2.0 | chitin | pathing | invalid; blocked-by not tracked |
| 95 | GetBlockingDoor | stub | Object | 2 | 1 | 1 | 2.0 | chitin | pathing | invalid; blocked-by not tracked |
| 96 | GetCreatureMovmentType | stub | Int | 1 | 1 | 1 | 2.0 | chitin | - | returns 0 fixed |
| 97 | GetCreatureSize | stub | Int | 2 | 1 | 1 | 2.0 | chitin | - | returns 3 (medium) fixed |
| 98 | GetFoundEnemyCreature | stub | Object | 1 | 1 | 1 | 2.0 | chitin | rules | invalid |
| 99 | GetLastForfeitViolation | stub | Int | 4 | 1 | 1 | 2.0 | chitin | minigames | FALSE |
| 100 | GetSubRace | stub | Int | 4 | 1 | 1 | 2.0 | chitin | - | returns 0 fixed |
| 101 | IsFormActive | stub | Int | 9 | 1 | 1 | 2.0 | chitin | rules | FALSE |
| 102 | IsMeditating | stub | Int | 2 | 1 | 1 | 2.0 | chitin | rules | FALSE |
| 103 | ModifyFortitudeSavingThrowBase | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 104 | MusicBattleStop | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 105 | PauseGame | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 106 | SetCurrentForm | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 107 | SetMapPinEnabled | noop | Void | 2 | 2 | 2 | 2.0 | 402dxn | - | accepted and ignored |
| 108 | SetReturnStrref | noop | Void | 5 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 109 | SoundObjectSetPosition | noop | Void | 1 | 1 | 1 | 2.0 | chitin | audio | no-op; sound object position not applied |
| 110 | ActionSurrenderToEnemies | noop | Void | 2 | 1 | 1 | 1.0 | 403dxn | - | accepted and ignored |
| 111 | AmbientSoundPlay | noop | Void | 1 | 1 | 1 | 1.0 | 402dxn | audio | no-op; ambient sound not routed to audio |
| 112 | AmbientSoundStop | noop | Void | 1 | 1 | 1 | 1.0 | 402dxn | audio | no-op; ambient sound not routed to audio |
| 113 | DuplicateHeadAppearance | noop | Void | 1 | 1 | 1 | 1.0 | 711kor | presentation | no-op |
| 114 | GetIsImmune | stub | Int | 1 | 1 | 1 | 1.0 | 506ond | rules | FALSE; immunity not modelled |
| 115 | UnlockAllSongs | noop | Void | 1 | 1 | 1 | 1.0 | 904mal | - | no-op |
| 116 | YavinHackDoorClose | noop | Void | 1 | 1 | 1 | 1.0 | 303nar | minigames | no-op; minigame door |

## Every routine used by shipped scripts

Real routines are listed here with their status and call counts. Notes are filled only for the gaps above.

Destroyed objects, not gaps: DestroyObject, and a corpse destroyed 12 s after death (combat.md), make an object invisible to get and iter, so GetObjectByTag no longer finds it. SetIsDestroyable(FALSE) keeps a corpse. See world.md, 101per prison door.

| # | routine | status | returns | sites | scripts | modules | early | first | owner | note |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | GetObjectByTag | real | Object | 9215 | 2622 | 83 | 5492.0 | 001ebo | - |  |
| 2 | GetFirstPC | real | Object | 3401 | 1785 | 83 | 3695.0 | 001ebo | - |  |
| 3 | AssignCommand | real | Void | 7208 | 1729 | 81 | 3638.0 | 001ebo | - |  |
| 4 | GetScriptParameter | real | Int | 1763 | 1405 | 77 | 2972.0 | 001ebo | - |  |
| 5 | GetGlobalNumber | real | Int | 3321 | 1442 | 79 | 2955.0 | 001ebo | - |  |
| 6 | GetIsObjectValid | real | Int | 11196 | 1353 | 76 | 2778.0 | 001ebo | - |  |
| 7 | SetLocalBoolean | real | Void | 2090 | 1346 | 72 | 2659.0 | 001ebo | - |  |
| 8 | GetLocalBoolean | real | Int | 1757 | 1251 | 72 | 2407.0 | 001ebo | - |  |
| 9 | DelayCommand | real | Void | 5189 | 1187 | 82 | 2360.0 | 001ebo | - |  |
| 10 | SetGlobalNumber | real | Void | 1690 | 1042 | 76 | 2274.0 | 001ebo | - |  |
| 11 | ClearAllActions | real | Void | 4478 | 1021 | 76 | 2016.0 | 001ebo | - |  |
| 12 | ActionStartConversation | real | Void | 1050 | 816 | 77 | 1775.0 | 001ebo | - |  |
| 13 | GetLocation | real | Engine(2) | 2480 | 735 | 80 | 1442.0 | 001ebo | - |  |
| 14 | IntToString | real | String | 14770 | 730 | 72 | 1234.0 | 001ebo | - |  |
| 15 | ActionPlayAnimation | real | Void | 4127 | 650 | 73 | 1233.0 | 001ebo | - |  |
| 16 | GetWaypointByTag | real | Object | 3893 | 618 | 68 | 1117.0 | 001ebo | - |  |
| 17 | ActionDoCommand | real | Void | 3148 | 619 | 67 | 1091.0 | 001ebo | - |  |
| 18 | AurPostString | real | Void | 1686 | 562 | 72 | 1083.0 | 001ebo | - |  |
| 19 | DestroyObject | real | Void | 1121 | 506 | 66 | 1045.0 | 001ebo | - |  |
| 20 | GetPCSpeaker | real | Object | 601 | 399 | 48 | 1044.0 | 001ebo | - |  |
| 21 | IntToFloat | real | Float | 1657 | 568 | 70 | 1020.0 | 001ebo | - |  |
| 22 | ActionForceMoveToObject | real | Void | 682 | 531 | 68 | 960.0 | 001ebo | - |  |
| 23 | GetTag | real | String | 2021 | 526 | 64 | 949.0 | 001ebo | - |  |
| 24 | Random | real | Int | 8370 | 501 | 64 | 916.0 | 001ebo | - |  |
| 25 | GetEnteringObject | real | Object | 483 | 472 | 73 | 879.0 | 001ebo | - |  |
| 26 | GetLocalNumber | real | Int | 3568 | 495 | 66 | 879.0 | 001ebo | - |  |
| 27 | GetDistanceBetween | real | Float | 1477 | 479 | 62 | 851.0 | 001ebo | - |  |
| 28 | ActionMoveToObject | real | Void | 1147 | 479 | 70 | 849.0 | 001ebo | - |  |
| 29 | SetLocalNumber | real | Void | 3022 | 481 | 66 | 845.0 | 001ebo | - |  |
| 30 | GetStringLength | real | Int | 622 | 459 | 67 | 824.0 | 001ebo | - |  |
| 31 | ActionWait | real | Void | 584 | 461 | 66 | 821.0 | 001ebo | - |  |
| 32 | GetFacing | real | Float | 700 | 429 | 64 | 779.0 | 001ebo | - |  |
| 33 | SetFacing | real | Void | 473 | 440 | 66 | 778.0 | 001ebo | - |  |
| 34 | StringToInt | real | Int | 552 | 399 | 60 | 719.0 | 001ebo | - |  |
| 35 | ApplyEffectToObject | real | Void | 1745 | 371 | 71 | 715.0 | 001ebo | - |  |
| 36 | GetGender | real | Int | 782 | 396 | 63 | 715.0 | 001ebo | - |  |
| 37 | GetStringLeft | real | String | 406 | 403 | 62 | 715.0 | 001ebo | - |  |
| 38 | GetRacialType | real | Int | 2289 | 392 | 62 | 711.0 | 001ebo | - |  |
| 39 | d10 | real | Int | 396 | 396 | 62 | 708.0 | 001ebo | - |  |
| 40 | GetHasSpell | real | Int | 11678 | 400 | 62 | 704.0 | 001ebo | - |  |
| 41 | d3 | real | Int | 463 | 386 | 62 | 696.0 | 001ebo | - |  |
| 42 | d8 | real | Int | 386 | 383 | 62 | 687.0 | 001ebo | - |  |
| 43 | GetDistanceToObject2D | real | Float | 768 | 383 | 62 | 686.0 | 001ebo | - |  |
| 44 | GetDistanceBetween2D | real | Float | 383 | 382 | 62 | 685.0 | 001ebo | - |  |
| 45 | GetObjectPersonalSpace | real | Float | 382 | 382 | 62 | 685.0 | 001ebo | - |  |
| 46 | CreateObject | real | Object | 755 | 340 | 71 | 683.0 | 001ebo | - |  |
| 47 | ChangeToStandardFaction | real | Void | 813 | 327 | 65 | 639.0 | 001ebo | - |  |
| 48 | CreateItemOnObject | real | Object | 630 | 333 | 57 | 604.0 | 001ebo | - |  |
| 49 | GetStringRight | real | String | 342 | 339 | 60 | 600.0 | 001ebo | - |  |
| 50 | GetIsEncounterCreature | real | Int | 326 | 326 | 60 | 582.0 | 001ebo | - |  |
| 51 | SetListenPattern | real | Void | 2282 | 326 | 60 | 582.0 | 001ebo | - |  |
| 52 | SetListening | real | Void | 326 | 326 | 60 | 582.0 | 001ebo | - |  |
| 53 | GetItemPossessedBy | real | Object | 323 | 228 | 41 | 557.0 | 001ebo | - |  |
| 54 | SetGlobalFadeOut | real | Void | 291 | 255 | 69 | 489.0 | 001ebo | - |  |
| 55 | ActionOpenDoor | real | Void | 277 | 200 | 41 | 486.0 | 001ebo | - |  |
| 56 | IncrementGlobalNumber | real | Void | 351 | 267 | 50 | 485.0 | 003ebo | - |  |
| 57 | SetGlobalFadeIn | real | Void | 287 | 234 | 66 | 476.0 | 001ebo | - |  |
| 58 | GetPartyMemberByIndex | real | Object | 2029 | 250 | 54 | 461.0 | 001ebo | - |  |
| 59 | GetPartyLeader | real | Object | 488 | 246 | 59 | 441.0 | 001ebo | - |  |
| 60 | SetLocked | real | Void | 333 | 188 | 42 | 439.0 | 001ebo | - |  |
| 61 | ActionJumpToObject | real | Void | 492 | 223 | 48 | 426.0 | 003ebo | - |  |
| 62 | ExecuteScript | real | Void | 239 | 194 | 40 | 404.0 | 001ebo | - |  |
| 63 | SetFacingPoint | real | Void | 375 | 195 | 54 | 396.0 | 003ebo | - |  |
| 64 | FindSubString | real | Int | 235 | 227 | 48 | 388.0 | 004ebo | - |  |
| 65 | EffectVisualEffect | real | Engine(0) | 706 | 217 | 55 | 384.0 | 001ebo | - |  |
| 66 | GetModuleName | real | String | 250 | 217 | 44 | 382.0 | 003ebo | - |  |
| 67 | GetSubString | real | String | 473 | 219 | 47 | 379.0 | 004ebo | - |  |
| 68 | GetAbilityModifier | real | Int | 231 | 219 | 46 | 373.0 | 004ebo | - |  |
| 69 | IsAvailableCreature | real | Int | 437 | 208 | 44 | 356.0 | 004ebo | - |  |
| 70 | CancelCombat | real | Void | 323 | 217 | 52 | 348.0 | 001ebo | - |  |
| 71 | SetGlobalBoolean | real | Void | 289 | 153 | 45 | 346.0 | 001ebo | - |  |
| 72 | StartNewModule | real | Void | 184 | 165 | 59 | 319.0 | 001ebo | - |  |
| 73 | GetScriptStringParameter | real | String | 168 | 168 | 33 | 311.0 | 001ebo | - |  |
| 74 | ActionAttack | real | Void | 550 | 151 | 47 | 299.0 | 003ebo | - |  |
| 75 | GetPosition | real | Vector | 356 | 155 | 45 | 299.0 | 001ebo | - |  |
| 76 | SetCommandable | real | Void | 427 | 191 | 51 | 296.0 | 001ebo | - |  |
| 77 | ActionMoveToLocation | real | Void | 403 | 157 | 52 | 294.0 | 003ebo | - |  |
| 78 | GetGlobalBoolean | real | Int | 344 | 128 | 47 | 290.0 | 001ebo | - |  |
| 79 | AddJournalQuestEntry | real | Void | 328 | 142 | 35 | 281.0 | 001ebo | - |  |
| 80 | GetCurrentHitPoints | real | Int | 1676 | 158 | 55 | 281.0 | 001ebo | - |  |
| 81 | GetUserDefinedEventNumber | real | Int | 155 | 155 | 54 | 275.0 | 001ebo | - |  |
| 82 | GetNearestObjectByTag | real | Object | 173 | 163 | 50 | 265.0 | 001ebo | - |  |
| 83 | GetHasSkill | real | Int | 75 | 73 | 11 | 261.0 | 001ebo | - |  |
| 84 | GetSkillRank | real | Int | 146 | 125 | 22 | 260.0 | 001ebo | - |  |
| 85 | EffectDamage | real | Engine(0) | 267 | 147 | 47 | 259.0 | 001ebo | - |  |
| 86 | SetLockOrientationInDialog | real | Void | 223 | 121 | 40 | 246.0 | 003ebo | - |  |
| 87 | ActionJumpToLocation | real | Void | 272 | 123 | 47 | 239.0 | 001ebo | - |  |
| 88 | GetCommandable | real | Int | 158 | 148 | 47 | 235.0 | 001ebo | - |  |
| 89 | IsNPCPartyMember | real | Int | 208 | 108 | 37 | 229.0 | 003ebo | - |  |
| 90 | SetFadeUntilScript | real | Void | 122 | 112 | 49 | 229.0 | 001ebo | - |  |
| 91 | GetMaxHitPoints | real | Int | 773 | 132 | 45 | 228.0 | 003ebo | - |  |
| 92 | ActionCloseDoor | real | Void | 143 | 98 | 35 | 225.0 | 003ebo | - |  |
| 93 | GetHitDice | real | Int | 243 | 125 | 39 | 224.0 | 003ebo | - |  |
| 94 | GetItemInSlot | real | Object | 1110 | 120 | 46 | 212.0 | 001ebo | - |  |
| 95 | GetNumStackedItems | real | Int | 72 | 66 | 12 | 207.0 | 001ebo | - |  |
| 96 | EffectHeal | real | Engine(0) | 219 | 117 | 41 | 203.0 | 003ebo | - |  |
| 97 | GetIsPC | real | Int | 89 | 82 | 31 | 200.0 | 001ebo | - |  |
| 98 | GetFirstObjectInShape | real | Object | 485 | 117 | 37 | 197.0 | 003ebo | - |  |
| 99 | GetNextObjectInShape | real | Object | 483 | 114 | 35 | 194.0 | 003ebo | - |  |
| 100 | GetIsEnemy | real | Int | 613 | 116 | 35 | 187.0 | 003ebo | - |  |
| 101 | EffectBeam | inert | Engine(0) | 220 | 116 | 37 | 184.0 | 003ebo | presentation | visual beam only, applied as a value; no gameplay expected |
| 102 | GetPositionFromLocation | real | Vector | 133 | 86 | 37 | 180.0 | 003ebo | - |  |
| 103 | Location | real | Engine(2) | 283 | 108 | 42 | 178.0 | 001ebo | - |  |
| 104 | ActionPauseConversation | real | Void | 106 | 79 | 36 | 176.0 | 101per | - |  |
| 105 | ActionResumeConversation | real | Void | 106 | 79 | 36 | 176.0 | 101per | - |  |
| 106 | GetStandardFaction | real | Int | 543 | 110 | 39 | 176.0 | 003ebo | - |  |
| 107 | SoundObjectPlay | real | Void | 245 | 89 | 29 | 176.0 | 001ebo | - |  |
| 108 | GetJournalEntry | real | Int | 263 | 133 | 28 | 173.0 | 001ebo | - |  |
| 109 | IsStealthed | real | Int | 100 | 100 | 37 | 167.0 | 003ebo | - |  |
| 110 | GetNearestCreature | real | Object | 1541 | 98 | 34 | 162.0 | 003ebo | - |  |
| 111 | ApplyEffectAtLocation | real | Void | 110 | 86 | 30 | 161.0 | 001ebo | - |  |
| 112 | FloatToInt | real | Int | 116 | 81 | 20 | 160.0 | 001ebo | - |  |
| 113 | GetIsPartyLeader | real | Int | 147 | 147 | 25 | 156.0 | 103per | - |  |
| 114 | GetIsOpen | real | Int | 88 | 86 | 36 | 153.0 | 003ebo | - |  |
| 115 | GetHasFeat | real | Int | 823 | 89 | 33 | 151.0 | 003ebo | - |  |
| 116 | PrintString | real | Void | 98 | 90 | 35 | 151.0 | 003ebo | - |  |
| 117 | GetEffectType | real | Int | 482 | 88 | 34 | 150.0 | 003ebo | - |  |
| 118 | GetFirstEffect | real | Engine(0) | 406 | 88 | 34 | 150.0 | 003ebo | - |  |
| 119 | GetIsEffectValid | real | Int | 408 | 88 | 34 | 150.0 | 003ebo | - |  |
| 120 | GetNextEffect | real | Engine(0) | 406 | 88 | 34 | 150.0 | 003ebo | - |  |
| 121 | RemoveEffect | real | Void | 248 | 88 | 34 | 150.0 | 003ebo | - |  |
| 122 | SetMinOneHP | real | Void | 183 | 81 | 25 | 148.0 | 001ebo | - |  |
| 123 | IsObjectPartyMember | real | Int | 1736 | 90 | 37 | 147.0 | 003ebo | - |  |
| 124 | GiveXPToCreature | real | Void | 133 | 64 | 26 | 145.0 | 101per | - |  |
| 125 | ShipBuild | stub | Int | 103 | 88 | 35 | 145.0 | 003ebo | - | returns 1 = shipping build (nwscript 761); correct default |
| 126 | GetDistanceToObject | real | Float | 183 | 86 | 34 | 143.0 | 003ebo | - |  |
| 127 | SetPartyLeader | real | Int | 67 | 65 | 41 | 143.0 | 003ebo | - |  |
| 128 | GetIsFriend | real | Int | 194 | 88 | 34 | 142.0 | 003ebo | - |  |
| 129 | GetFirstItemInInventory | real | Object | 169 | 86 | 36 | 141.0 | 001ebo | - |  |
| 130 | ClearAllEffects | real | Void | 123 | 69 | 31 | 140.0 | 001ebo | scripts (done) | now clears the caller's applied effects (was a no-op) |
| 131 | GetCurrentAction | real | Int | 489 | 83 | 33 | 139.0 | 003ebo | - |  |
| 132 | GetNextItemInInventory | real | Object | 162 | 84 | 35 | 139.0 | 001ebo | - |  |
| 133 | ObjectToString | real | String | 245 | 86 | 35 | 137.0 | 003ebo | - |  |
| 134 | d4 | real | Int | 160 | 82 | 33 | 137.0 | 003ebo | - |  |
| 135 | GetIsConversationActive | real | Int | 87 | 84 | 34 | 136.0 | 003ebo | - |  |
| 136 | GetIsInCombat | real | Int | 471 | 83 | 34 | 134.0 | 003ebo | - |  |
| 137 | GetObjectSeen | real | Int | 668 | 83 | 34 | 134.0 | 003ebo | - |  |
| 138 | GetLevelByClass | real | Int | 2399 | 81 | 33 | 133.0 | 003ebo | - |  |
| 139 | GetObjectType | real | Int | 327 | 85 | 32 | 133.0 | 003ebo | - |  |
| 140 | GetRandomDestination | real | Vector | 235 | 80 | 34 | 133.0 | 003ebo | - |  |
| 141 | ActionUseTalentOnObject | real | Void | 1162 | 83 | 33 | 132.0 | 003ebo | - |  |
| 142 | GetHasSpellEffect | real | Int | 1494 | 82 | 34 | 131.0 | 003ebo | - |  |
| 143 | EffectParalyze | partial | Engine(0) | 83 | 81 | 32 | 130.0 | 003ebo | rules | debilitates via powers.rs DEBILITATING list; no other effect behaviour |
| 144 | d6 | real | Int | 316 | 82 | 32 | 130.0 | 003ebo | - |  |
| 145 | ActionEquipMostDamagingRanged | real | Void | 82 | 79 | 34 | 129.0 | 003ebo | - |  |
| 146 | GetCurrentForcePoints | real | Int | 80 | 80 | 32 | 129.0 | 003ebo | - |  |
| 147 | TalentSpell | real | Engine(3) | 8630 | 80 | 33 | 129.0 | 003ebo | - |  |
| 148 | EffectSleep | partial | Engine(0) | 79 | 79 | 32 | 128.0 | 003ebo | rules | debilitates via powers.rs DEBILITATING list |
| 149 | FloatToString | real | String | 243 | 80 | 35 | 128.0 | 003ebo | - |  |
| 150 | GetName | real | String | 157 | 80 | 32 | 128.0 | 003ebo | - |  |
| 151 | ActionMoveAwayFromObject | real | Void | 156 | 79 | 32 | 127.0 | 003ebo | - |  |
| 152 | GetAttemptedAttackTarget | real | Object | 111 | 79 | 32 | 127.0 | 003ebo | - |  |
| 153 | GetCreatureHasTalent | real | Int | 4084 | 78 | 33 | 127.0 | 003ebo | - |  |
| 154 | GetIsDead | real | Int | 927 | 79 | 32 | 127.0 | 003ebo | - |  |
| 155 | GetSoloMode | real | Int | 327 | 80 | 34 | 127.0 | 003ebo | - |  |
| 156 | GetBaseItemType | real | Int | 2853 | 80 | 33 | 126.0 | 003ebo | - |  |
| 157 | GetNPCAIStyle | real | Int | 924 | 80 | 32 | 126.0 | 003ebo | - |  |
| 158 | TalentFeat | real | Engine(3) | 159 | 80 | 32 | 126.0 | 003ebo | - |  |
| 159 | ActionFollowLeader | real | Void | 233 | 78 | 32 | 125.0 | 003ebo | - | walks after the party leader; yields to the next queued action (working interpretation, world.md Party follow); repeats do not stack |
| 160 | ActionFollowOwner | real | Void | 78 | 78 | 32 | 125.0 | 003ebo | - | walks after Object::owner (nothing sets it yet); holds the queue until ClearAllActions (nwscript 843) |
| 161 | GetAttackTarget | real | Object | 429 | 78 | 32 | 125.0 | 003ebo | - |  |
| 162 | GetEffectSpellId | real | Int | 111 | 78 | 32 | 125.0 | 003ebo | - |  |
| 163 | GetIsPuppet | stub | Int | 155 | 78 | 32 | 125.0 | 003ebo | scripts | false for everything; puppets (PUP table) not modelled; false is right unless puppets exist |
| 164 | GetPUPOwner | partial | Object | 155 | 78 | 32 | 125.0 | 003ebo | scripts | invalid unless Object::owner is a party member (nwscript 841); nothing sets Object::owner yet (AssignPUP, SpawnAvailablePUP are stubs) |
| 165 | HasLineOfSight | real | Int | 78 | 78 | 32 | 125.0 | 003ebo | - |  |
| 166 | d100 | real | Int | 463 | 78 | 32 | 125.0 | 003ebo | - |  |
| 167 | ActionEquipMostDamagingMelee | real | Void | 79 | 78 | 32 | 124.0 | 003ebo | - |  |
| 168 | ActionSwitchWeapons | real | Void | 154 | 77 | 32 | 123.0 | 003ebo | scripts (done) | now queued and swaps the two weapon sets when reached (not in dialog) |
| 169 | GetCategoryFromTalent | real | Int | 154 | 77 | 32 | 123.0 | 003ebo | - |  |
| 170 | GetCreatureTalentBest | real | Engine(3) | 693 | 77 | 32 | 123.0 | 003ebo | - |  |
| 171 | GetCreatureTalentRandom | real | Engine(3) | 3003 | 77 | 32 | 123.0 | 003ebo | - |  |
| 172 | GetHealTarget | stub | Object | 77 | 77 | 32 | 123.0 | 003ebo | rules | invalid; healer AI target not stored (nwscript 814) |
| 173 | GetIdFromTalent | real | Int | 2695 | 77 | 32 | 123.0 | 003ebo | - |  |
| 174 | GetIsDebilitated | real | Int | 308 | 77 | 32 | 123.0 | 003ebo | - | real |
| 175 | GetIsTalentValid | real | Int | 4466 | 77 | 32 | 123.0 | 003ebo | - |  |
| 176 | GetLastAttackAction | real | Int | 77 | 77 | 32 | 123.0 | 003ebo | - |  |
| 177 | GetLastAttackResult | real | Int | 77 | 77 | 32 | 123.0 | 003ebo | - |  |
| 178 | GetLastCombatFeatUsed | real | Int | 77 | 77 | 32 | 123.0 | 003ebo | - |  |
| 179 | GetLastForcePowerUsed | real | Int | 154 | 77 | 32 | 123.0 | 003ebo | - |  |
| 180 | GetLastHostileActor | real | Object | 95 | 77 | 32 | 123.0 | 003ebo | - |  |
| 181 | GetLastHostileTarget | real | Object | 308 | 77 | 32 | 123.0 | 003ebo | - |  |
| 182 | GetPartyAIStyle | real | Int | 168 | 77 | 32 | 123.0 | 003ebo | - |  |
| 183 | GetPlayerRestrictMode | stub | Int | 185 | 77 | 32 | 123.0 | 003ebo | hud | always FALSE |
| 184 | GetSpellBaseForcePointCost | real | Int | 77 | 77 | 32 | 123.0 | 003ebo | - |  |
| 185 | GetTypeFromTalent | real | Int | 385 | 77 | 32 | 123.0 | 003ebo | - |  |
| 186 | GetUserActionsPending | stub | Int | 91 | 77 | 32 | 123.0 | 003ebo | rules | always FALSE (combat queue not modelled) |
| 187 | GetWasForcePowerSuccessful | real | Int | 77 | 77 | 32 | 123.0 | 003ebo | - |  |
| 188 | SetHealTarget | noop | Void | 308 | 77 | 32 | 123.0 | 003ebo | rules | accepted and dropped; healer AI target not stored (nwscript 813) |
| 189 | AddPartyMember | real | Int | 66 | 50 | 27 | 118.0 | 001ebo | - |  |
| 190 | EffectDeath | real | Engine(0) | 67 | 59 | 36 | 118.0 | 003ebo | - |  |
| 191 | SpawnAvailableNPC | real | Object | 83 | 51 | 29 | 109.0 | 003ebo | - |  |
| 192 | RevealMap | noop | Void | 38 | 38 | 29 | 103.0 | 001ebo | hud | no-op; map fog of war not modelled |
| 193 | GetItemStackSize | real | Int | 51 | 49 | 14 | 100.0 | 001ebo | - |  |
| 194 | SetItemStackSize | real | Void | 53 | 49 | 16 | 99.0 | 001ebo | - |  |
| 195 | PlayRoomAnimation | real | Void | 91 | 33 | 17 | 93.0 | 003ebo | - |  |
| 196 | SignalEvent | real | Void | 167 | 36 | 11 | 93.0 | 004ebo | - |  |
| 197 | SwitchPlayerCharacter | real | Int | 45 | 43 | 19 | 93.0 | 001ebo | - |  |
| 198 | EventUserDefined | real | Engine(1) | 81 | 34 | 11 | 89.0 | 004ebo | - |  |
| 199 | AddAvailableNPCByObject | real | Int | 33 | 33 | 17 | 88.0 | 001ebo | - |  |
| 200 | GetLoadFromSaveGame | stub | Int | 43 | 43 | 41 | 88.0 | 001ebo | scripts | always FALSE; correct here because World::load runs no scripts |
| 201 | SetCreatureAILevel | noop | Void | 78 | 38 | 19 | 82.0 | 003ebo | rules | no-op; creature AI priority not modelled |
| 202 | ActionUnlockObject | real | Void | 44 | 39 | 15 | 80.0 | 103per | - |  |
| 203 | ActionForceMoveToLocation | real | Void | 102 | 44 | 23 | 79.0 | 152har | - |  |
| 204 | PlayMovie | real | Void | 60 | 33 | 21 | 78.0 | 001ebo | - |  |
| 205 | RemoveNPCFromPartyToBase | real | Int | 40 | 38 | 25 | 78.0 | 003ebo | - |  |
| 206 | GetCombatActionsPending | real | Int | 55 | 55 | 28 | 77.0 | 103per | - |  |
| 207 | GetNextPC | real | Object | 76 | 38 | 23 | 77.0 | 101per | scripts (done) | now walks the party in order (was invalid after the leader) |
| 208 | GetGoodEvilValue | real | Int | 83 | 49 | 18 | 76.0 | 003ebo | - |  |
| 209 | SetNPCAIStyle | real | Void | 41 | 35 | 16 | 76.0 | 003ebo | - |  |
| 210 | ShowTutorialWindow | noop | Void | 21 | 19 | 5 | 76.0 | 001ebo | hud | no-op; tutorial popups (tutorial.2da) not shown |
| 211 | SWMG_PlayAnimation | real | Void | 222 | 52 | 5 | 74.0 | 211tel | - |  |
| 212 | DisableVideoEffect | noop | Void | 23 | 23 | 12 | 72.0 | 001ebo | presentation | no-op; frame-buffer video effects not applied |
| 213 | SetNPCSelectability | real | Void | 53 | 34 | 21 | 72.0 | 003ebo | - |  |
| 214 | EnableVideoEffect | noop | Void | 22 | 22 | 17 | 70.0 | 001ebo | presentation | no-op; frame-buffer video effects not applied |
| 215 | GetAbilityScore | real | Int | 40 | 31 | 5 | 70.0 | 003ebo | - |  |
| 216 | GetSpellAcquired | real | Int | 94 | 35 | 19 | 68.0 | 003ebo | - |  |
| 217 | OpenStore | real | Void | 45 | 36 | 17 | 68.0 | 201tel | - |  |
| 218 | SetKeepStealthInDialog | real | Void | 34 | 33 | 13 | 68.0 | 101per | - |  |
| 219 | GetGold | real | Int | 32 | 30 | 12 | 65.0 | 201tel | - |  |
| 220 | GetModule | real | Object | 44 | 36 | 8 | 65.0 | 102per | - |  |
| 221 | BarkString | real | Void | 45 | 33 | 14 | 64.0 | 001ebo | - |  |
| 222 | SoundObjectStop | real | Void | 41 | 24 | 16 | 64.0 | 101per | - |  |
| 223 | SetOrientOnClick | noop | Void | 46 | 43 | 12 | 63.0 | 003ebo | hud | no-op; creatures do not turn to the player on click |
| 224 | GetArea | real | Object | 51 | 27 | 11 | 61.0 | 001ebo | - |  |
| 225 | NoClicksFor | real | Void | 49 | 36 | 20 | 58.0 | 003ebo | - |  |
| 226 | SetAreaUnescapable | real | Void | 32 | 27 | 10 | 58.0 | 203tel | scripts (done) | now refuses party module transitions (triggers, linked doors) until cleared |
| 227 | SoundObjectFadeAndStop | real | Void | 73 | 27 | 10 | 52.0 | 001ebo | - |  |
| 228 | ActionLockObject | real | Void | 26 | 22 | 11 | 51.0 | 201tel | - |  |
| 229 | SetCustomToken | real | Void | 123 | 33 | 16 | 51.0 | 001ebo | - |  |
| 230 | SetDialogPlaceableCamera | noop | Void | 65 | 26 | 16 | 51.0 | 003ebo | hud | no-op; dialog camera cut to placeable not implemented |
| 231 | GetLastUsedBy | real | Object | 29 | 18 | 12 | 50.0 | 001ebo | - |  |
| 232 | PlayAnimation | real | Void | 104 | 34 | 21 | 49.0 | 103per | - |  |
| 233 | GetPartyMemberCount | real | Int | 29 | 24 | 13 | 48.0 | 103per | - |  |
| 234 | ReflexSave | real | Int | 46 | 22 | 11 | 48.0 | 101per | - |  |
| 235 | RemovePartyMember | real | Int | 99 | 21 | 13 | 47.0 | 153har | - |  |
| 236 | GetIsXBox | stub | Int | 18 | 16 | 11 | 46.0 | 001ebo | - | FALSE (PC build); correct |
| 237 | PlaySound | real | Void | 17 | 16 | 9 | 46.0 | 001ebo | - |  |
| 238 | EffectPoison | inert | Engine(0) | 34 | 22 | 14 | 45.0 | 101per | rules | inert: no poison damage over time |
| 239 | SurrenderToEnemies | noop | Void | 52 | 30 | 14 | 45.0 | 003ebo | rules | no-op; combat surrender not modelled |
| 240 | EffectMovementSpeedDecrease | real | Engine(0) | 33 | 25 | 14 | 44.0 | 003ebo | - |  |
| 241 | SWMG_GetPlayerSpeed | real | Float | 29 | 26 | 3 | 44.0 | 211tel | - |  |
| 242 | GetIsNeutral | real | Int | 48 | 20 | 13 | 42.0 | 101per | - |  |
| 243 | TakeGoldFromCreature | real | Void | 61 | 23 | 14 | 42.0 | 202tel | - |  |
| 244 | GiveGoldToCreature | real | Void | 55 | 26 | 16 | 41.0 | 203tel | - |  |
| 245 | ShowUpgradeScreen | real | Void | 15 | 15 | 9 | 41.0 | 001ebo | - |  |
| 246 | SWMG_SetPlayerAccelerationPerSecond | real | Void | 108 | 24 | 3 | 40.0 | 211tel | - |  |
| 247 | SWMG_GetPlayer | real | Object | 47 | 31 | 5 | 39.0 | 211tel | - |  |
| 248 | SetEncounterActive | real | Void | 58 | 18 | 10 | 39.0 | 101per | - | encounter.rs: sets the Active flag that gates spawns (scripts still fire); see world.md Encounters |
| 249 | RemoveHeartbeat | real | Void | 19 | 19 | 1 | 38.0 | chitin | scripts (done) | now drops the placeable OnHeartbeat script |
| 250 | AdjustCreatureAttributes | noop | Void | 28 | 16 | 3 | 37.0 | 003ebo | rules | no-op; ability adjust not applied (stat formulas) |
| 251 | GetLastPazaakResult | real | Int | 25 | 17 | 7 | 37.0 | 202tel | - |  |
| 252 | ActionEquipItem | real | Void | 42 | 18 | 12 | 36.0 | 003ebo | - |  |
| 253 | CreatureFlourishWeapon | noop | Void | 33 | 22 | 16 | 36.0 | 001ebo | presentation | no-op; cosmetic flourish |
| 254 | SetIsDestroyable | real | Void | 16 | 14 | 7 | 36.0 | 001ebo | - |  |
| 255 | GetInfluence | real | Int | 48 | 17 | 6 | 35.0 | 003ebo | - |  |
| 256 | GetNPCSelectability | real | Int | 48 | 17 | 8 | 35.0 | 003ebo | - |  |
| 257 | SWMG_SetPlayerSpeed | real | Void | 21 | 21 | 3 | 35.0 | 211tel | - |  |
| 258 | SetForfeitConditions | noop | Void | 23 | 15 | 4 | 35.0 | 003ebo | minigames | no-op; minigame forfeit rules |
| 259 | AdjustAlignment | real | Void | 25 | 24 | 13 | 34.0 | 203tel | - |  |
| 260 | EffectResurrection | real | Engine(0) | 18 | 15 | 10 | 33.0 | 004ebo | - |  |
| 261 | GetFirstObjectInArea | real | Object | 30 | 26 | 13 | 33.0 | 004ebo | - |  |
| 262 | BeginConversation | real | Int | 83 | 15 | 2 | 32.0 | 102per | - |  |
| 263 | GetLastSpeaker | real | Object | 16 | 15 | 2 | 32.0 | 102per | - |  |
| 264 | GetListenPatternNumber | real | Int | 16 | 15 | 2 | 32.0 | 102per | - |  |
| 265 | GetNextObjectInArea | real | Object | 40 | 25 | 12 | 32.0 | 004ebo | - |  |
| 266 | GetSpellTarget | real | Object | 17 | 16 | 1 | 32.0 | chitin | - |  |
| 267 | PlayPazaak | real | Void | 21 | 16 | 10 | 32.0 | 003ebo | - |  |
| 268 | ActionBarkString | real | Void | 30 | 19 | 10 | 31.0 | 101per | - |  |
| 269 | CutsceneAttack | real | Void | 25 | 14 | 10 | 31.0 | 003ebo | - |  |
| 270 | SWMG_SetPlayerMinSpeed | real | Void | 58 | 19 | 3 | 31.0 | 211tel | - |  |
| 271 | GetAttemptedSpellTarget | stub | Object | 32 | 15 | 1 | 30.0 | chitin | rules | invalid; combat target not stored |
| 272 | GetNearestObject | real | Object | 46 | 23 | 13 | 30.0 | 101per | - |  |
| 273 | ActionUnequipItem | real | Void | 25 | 12 | 10 | 29.0 | 001ebo | - |  |
| 274 | EffectDisguise | inert | Engine(0) | 11 | 11 | 8 | 29.0 | 001ebo | rules | inert |
| 275 | Vector | real | Vector | 25 | 22 | 12 | 29.0 | 001ebo | - |  |
| 276 | ResetDialogState | noop | Void | 16 | 14 | 1 | 28.0 | chitin | dialog | no-op; engine dialog global state not kept |
| 277 | SWMG_GetSwoopUpgrade | real | Int | 34 | 18 | 3 | 28.0 | 211tel | - |  |
| 278 | ActionTakeItem | real | Void | 46 | 21 | 13 | 27.0 | 203tel | - |  |
| 279 | AddAvailableNPCByTemplate | real | Int | 31 | 14 | 10 | 27.0 | 001ebo | - |  |
| 280 | SWMG_SetPlayerMaxSpeed | real | Void | 55 | 16 | 3 | 26.0 | 211tel | - |  |
| 281 | SetLightsaberPowered | noop | Void | 47 | 19 | 13 | 25.0 | 262tel | presentation | no-op; blade on/off override not shown |
| 282 | ShowPartySelectionGUI | real | Void | 18 | 15 | 11 | 24.0 | 003ebo | - |  |
| 283 | GetFirstInPersistentObject | real | Object | 7 | 7 | 5 | 23.0 | 104per | - |  |
| 284 | GetNextInPersistentObject | real | Object | 7 | 7 | 5 | 23.0 | 104per | - |  |
| 285 | GrantSpell | real | Void | 22 | 15 | 9 | 23.0 | 101per | - |  |
| 286 | PlayMovieQueue | noop | Void | 14 | 10 | 6 | 23.0 | 003ebo | hud | no-op; QueueMovie/PlayMovie route through WorldEvent::Movie instead |
| 287 | RemoveEffectByID | real | Void | 13 | 13 | 10 | 23.0 | 001ebo | scripts (done) | now removes applied effects of the type |
| 288 | SWMG_GetPosition | real | Vector | 25 | 15 | 5 | 23.0 | 211tel | - |  |
| 289 | SetFakeCombatState | noop | Void | 27 | 12 | 9 | 23.0 | 153har | presentation | no-op; fake combat flag not kept |
| 290 | d20 | real | Int | 15 | 15 | 12 | 23.0 | 101per | - |  |
| 291 | GetLevelByPosition | real | Int | 15 | 14 | 6 | 22.0 | 207tel | - |  |
| 292 | GiveItem | real | Void | 42 | 11 | 8 | 22.0 | 003ebo | - |  |
| 293 | SetLockHeadFollowInDialog | noop | Void | 15 | 10 | 7 | 22.0 | 103per | presentation | no-op; head follow lock in dialog |
| 294 | EffectChoke | partial | Engine(0) | 25 | 13 | 10 | 21.0 | 101per | rules | debilitates via powers.rs DEBILITATING list |
| 295 | SetSoloMode | real | Void | 9 | 9 | 8 | 21.0 | 004ebo | - |  |
| 296 | EffectMovementSpeedIncrease | real | Engine(0) | 23 | 11 | 8 | 20.0 | 003ebo | - |  |
| 297 | RemoveAvailableNPC | real | Int | 32 | 10 | 9 | 20.0 | 001ebo | - |  |
| 298 | SWMG_SetPlayerTunnelNeg | real | Void | 12 | 12 | 3 | 20.0 | 211tel | - |  |
| 299 | SWMG_SetPlayerTunnelPos | real | Void | 12 | 12 | 3 | 20.0 | 211tel | - |  |
| 300 | GetLastPerceived | real | Object | 15 | 7 | 4 | 19.0 | 101per | - |  |
| 301 | GetLocked | real | Int | 5 | 5 | 3 | 19.0 | 101per | - |  |
| 302 | GivePlotXP | real | Void | 20 | 16 | 4 | 19.0 | 106per | - |  |
| 303 | EffectForcePushTargeted | partial | Engine(0) | 17 | 13 | 10 | 18.0 | 101per | rules | push effect; movement part not modelled |
| 304 | EffectLinkEffects | real | Engine(0) | 226 | 10 | 4 | 18.0 | 107per | - |  |
| 305 | EffectStunned | partial | Engine(0) | 13 | 8 | 5 | 18.0 | 102per | rules | debilitates via powers.rs DEBILITATING list |
| 306 | SWMG_GetIsInvulnerable | real | Int | 10 | 10 | 3 | 18.0 | 211tel | - |  |
| 307 | GetLastPerceptionSeen | real | Int | 10 | 6 | 4 | 17.0 | 101per | - |  |
| 308 | EnableRendering | noop | Void | 9 | 9 | 9 | 16.0 | 001ebo | presentation | no-op; object rendering toggle not implemented |
| 309 | GetLastAttacker | real | Object | 8 | 6 | 3 | 16.0 | 101per | - |  |
| 310 | SetForceAlwaysUpdate | noop | Void | 27 | 13 | 7 | 16.0 | 222tel | scripts (open) | no-op; far-object update override not implemented |
| 311 | QueueMovie | real | Void | 45 | 8 | 6 | 15.0 | 003ebo | - |  |
| 312 | SWMG_OnDeath | real | Void | 15 | 15 | 2 | 15.0 | 421dxn | - |  |
| 313 | SWMG_SetLateralAccelerationPerSecond | real | Void | 15 | 9 | 3 | 15.0 | 211tel | - |  |
| 314 | SpawnAvailablePUP | stub | Object | 6 | 6 | 6 | 15.0 | 003ebo | scripts | invalid; puppets not modelled |
| 315 | StopRumblePattern | noop | Int | 4 | 4 | 2 | 15.0 | 103per | - | no-op; controller rumble not implemented (fine) |
| 316 | EffectCutSceneHorrified | inert | Engine(0) | 7 | 5 | 5 | 14.0 | 105per | presentation | inert; cutscene visual |
| 317 | EffectHealForcePoints | real | Engine(0) | 12 | 10 | 2 | 14.0 | chitin | - |  |
| 318 | JumpToObject | real | Void | 14 | 8 | 8 | 14.0 | 106per | - |  |
| 319 | MusicBackgroundPlay | noop | Void | 7 | 6 | 4 | 14.0 | 101per | audio | no-op; scripted background music not routed to audio |
| 320 | SWMG_GetPlayerAccelerationPerSecond | real | Float | 8 | 8 | 3 | 14.0 | 211tel | - |  |
| 321 | ShowChemicalUpgradeScreen | real | Void | 5 | 5 | 4 | 14.0 | 101per | - |  |
| 322 | EffectCutSceneParalyze | inert | Engine(0) | 5 | 5 | 5 | 13.0 | 101per | presentation | inert; cutscene visual |
| 323 | GetRunScriptVar | real | Int | 5 | 5 | 3 | 13.0 | 003ebo | - |  |
| 324 | MusicBackgroundStop | noop | Void | 5 | 5 | 3 | 13.0 | 101per | audio | no-op; scripted background music not routed to audio |
| 325 | PlayRumblePattern | stub | Int | 4 | 4 | 4 | 13.0 | 003ebo | - | returns 0; no controller rumble (fine) |
| 326 | SWMG_GetHitPoints | real | Int | 13 | 9 | 5 | 13.0 | 211tel | - |  |
| 327 | SetPlayerRestrictMode | noop | Void | 7 | 7 | 5 | 13.0 | 104per | hud | no-op; no input gate |
| 328 | GetClassByPosition | real | Int | 12 | 7 | 3 | 12.0 | chitin | - |  |
| 329 | GetSpellId | real | Int | 140 | 6 | 1 | 12.0 | chitin | - |  |
| 330 | JumpToLocation | real | Void | 14 | 6 | 4 | 12.0 | 101per | - |  |
| 331 | SetEffectIcon | real | Engine(0) | 110 | 6 | 1 | 12.0 | chitin | - |  |
| 332 | SetPlanetAvailable | real | Void | 16 | 3 | 2 | 12.0 | 001ebo | - |  |
| 333 | SetPlanetSelectable | real | Void | 17 | 3 | 2 | 12.0 | 001ebo | - |  |
| 334 | AddBonusForcePoints | real | Void | 7 | 6 | 4 | 11.0 | 204tel | - |  |
| 335 | EffectAssuredHit | inert | Engine(0) | 7 | 7 | 4 | 11.0 | 202tel | rules | inert; combat hit guarantee not applied |
| 336 | ForceHeartbeat | real | Void | 9 | 5 | 3 | 11.0 | 101per | - |  |
| 337 | GetExitingObject | real | Object | 8 | 8 | 7 | 11.0 | 262tel | - |  |
| 338 | ResetCreatureAILevel | noop | Void | 41 | 5 | 5 | 11.0 | 203tel | rules | no-op; creature AI priority not modelled |
| 339 | SWMG_DestroyMiniGameObject | real | Void | 7 | 7 | 3 | 11.0 | 211tel | - |  |
| 340 | SWMG_GetMaxHitPoints | real | Int | 9 | 7 | 3 | 11.0 | 211tel | - |  |
| 341 | SWMG_SetFollowerHitPoints | real | Void | 9 | 7 | 3 | 11.0 | 211tel | - |  |
| 342 | SWMG_StartInvulnerability | real | Void | 5 | 5 | 3 | 11.0 | 211tel | - |  |
| 343 | EffectDamageIncrease | real | Engine(0) | 15 | 7 | 5 | 10.0 | chitin | - |  |
| 344 | EffectDroidStun | partial | Engine(0) | 9 | 4 | 2 | 10.0 | 103per | rules | debilitates via powers.rs DEBILITATING list |
| 345 | EffectHorrified | partial | Engine(0) | 7 | 4 | 2 | 10.0 | 107per | rules | debilitates via powers.rs DEBILITATING list |
| 346 | GetSpellTargetObject | real | Object | 28 | 5 | 1 | 10.0 | chitin | - |  |
| 347 | ModifyInfluence | real | Void | 9 | 7 | 3 | 10.0 | chitin | - |  |
| 348 | SWMG_AdjustFollowerHitPoints | real | Int | 6 | 6 | 3 | 10.0 | 211tel | - |  |
| 349 | EffectAttackIncrease | real | Engine(0) | 20 | 6 | 4 | 9.0 | chitin | - |  |
| 350 | SWMG_GetLastFollowerHit | real | Object | 5 | 5 | 3 | 9.0 | 211tel | - |  |
| 351 | SetInputClass | noop | Void | 4 | 4 | 4 | 9.0 | 107per | hud | no-op; input class (dialog, minigame, GUI) not routed |
| 352 | WillSave | real | Int | 8 | 5 | 2 | 9.0 | chitin | - |  |
| 353 | d2 | real | Int | 9 | 6 | 3 | 9.0 | chitin | - |  |
| 354 | ChangeObjectAppearance | real | Void | 3 | 3 | 3 | 8.0 | 107per | - |  |
| 355 | DisableHealthRegen | noop | Void | 2 | 2 | 1 | 8.0 | 001ebo | rules | no-op; innate regen toggle not implemented |
| 356 | EffectAbilityIncrease | real | Engine(0) | 55 | 5 | 3 | 8.0 | chitin | - |  |
| 357 | GetOwnerDemolitionsSkill | real | Int | 8 | 4 | 4 | 8.0 | 101per | - |  |
| 358 | GetReflexAdjustedDamage | real | Int | 6 | 4 | 4 | 8.0 | 101per | rules | returns the damage unadjusted; reflex save halving not modelled |
| 359 | GetReputation | real | Int | 2 | 2 | 1 | 8.0 | 101per | - |  |
| 360 | GetTrapBaseType | real | Int | 11 | 6 | 4 | 8.0 | chitin | - |  |
| 361 | IsRunning | real | Int | 3 | 3 | 2 | 8.0 | 101per | - |  |
| 362 | ShowGalaxyMap | real | Void | 3 | 2 | 2 | 8.0 | 001ebo | - |  |
| 363 | EffectAttackDecrease | real | Engine(0) | 12 | 5 | 4 | 7.0 | chitin | - |  |
| 364 | GetAppearanceType | real | Int | 11 | 3 | 3 | 7.0 | 001ebo | - |  |
| 365 | GetGlobalLocation | real | Engine(2) | 17 | 7 | 4 | 7.0 | 403dxn | - |  |
| 366 | GetTimeHour | real | Int | 18 | 5 | 5 | 7.0 | 211tel | - |  |
| 367 | GetTimeMillisecond | real | Int | 13 | 5 | 5 | 7.0 | 211tel | - | real (time from module start hour) |
| 368 | GetTimeMinute | real | Int | 13 | 5 | 5 | 7.0 | 211tel | - |  |
| 369 | GetTimeSecond | real | Int | 13 | 5 | 5 | 7.0 | 211tel | - |  |
| 370 | SWMG_GetObjectName | real | String | 7 | 5 | 3 | 7.0 | 211tel | - |  |
| 371 | SWMG_GetPlayerOffset | real | Vector | 5 | 5 | 5 | 7.0 | 211tel | - |  |
| 372 | DecrementGlobalNumber | real | Void | 3 | 3 | 3 | 6.0 | 202tel | - |  |
| 373 | EffectACDecrease | real | Engine(0) | 9 | 3 | 1 | 6.0 | chitin | - |  |
| 374 | EffectAbilityDecrease | real | Engine(0) | 21 | 3 | 1 | 6.0 | chitin | - |  |
| 375 | EffectBlind | inert | Engine(0) | 2 | 2 | 2 | 6.0 | 102per | rules | inert |
| 376 | FortitudeSave | real | Int | 8 | 3 | 1 | 6.0 | chitin | - |  |
| 377 | GetIsInConversation | real | Int | 2 | 2 | 2 | 6.0 | 101per | - |  |
| 378 | GetIsPoisoned | stub | Int | 10 | 3 | 1 | 6.0 | chitin | rules | FALSE; poison not modelled |
| 379 | GetMaxForcePoints | real | Int | 3 | 3 | 1 | 6.0 | chitin | - |  |
| 380 | GetSpellTargetLocation | real | Engine(2) | 39 | 3 | 1 | 6.0 | chitin | - |  |
| 381 | GrantFeat | real | Void | 2 | 2 | 2 | 6.0 | 101per | - |  |
| 382 | MusicBattlePlay | noop | Void | 2 | 2 | 2 | 6.0 | 001ebo | audio | no-op; battle music not routed to audio |
| 383 | PlayOverlayAnimation | real | Void | 5 | 3 | 3 | 6.0 | 103per | - |  |
| 384 | RoundsToSeconds | real | Float | 3 | 3 | 1 | 6.0 | chitin | - |  |
| 385 | SWMG_PlayerApplyForce | real | Void | 6 | 6 | 2 | 6.0 | 371nar | - |  |
| 386 | SetDisableTransit | real | Void | 2 | 2 | 2 | 6.0 | 261tel | scripts (done) | now refuses party module transitions until cleared |
| 387 | SetPlotFlag | real | Void | 24 | 6 | 1 | 6.0 | 711kor | - |  |
| 388 | ShowSwoopUpgradeScreen | real | Void | 2 | 2 | 2 | 6.0 | 003ebo | - |  |
| 389 | ActionUseSkill | partial | Void | 8 | 2 | 2 | 5.0 | 101per | - | security, stealth and demolitions (with a trap) are answered by craft, stealth and traps; other skills accepted and ignored |
| 390 | AddAvailablePUPByObject | stub | Int | 2 | 2 | 2 | 5.0 | 231tel | scripts | TRUE; puppets not modelled |
| 391 | AssignPUP | stub | Int | 2 | 2 | 2 | 5.0 | 231tel | scripts | returns 0; puppets not modelled |
| 392 | GetPlotFlag | real | Int | 8 | 3 | 2 | 5.0 | chitin | - |  |
| 393 | ModifyWillSavingThrowBase | noop | Void | 3 | 3 | 2 | 5.0 | chitin | rules | no-op; saving throw base not adjusted |
| 394 | SWMG_GetPlayerMinSpeed | real | Float | 3 | 3 | 3 | 5.0 | 211tel | - |  |
| 395 | SWMG_GetSphereRadius | real | Float | 6 | 3 | 3 | 5.0 | 211tel | - |  |
| 396 | SWMG_GetTrackPosition | real | Vector | 3 | 3 | 3 | 5.0 | 211tel | - |  |
| 397 | SWMG_RemoveAnimation | real | Void | 15 | 3 | 3 | 5.0 | 211tel | - |  |
| 398 | SWMG_SetFollowerPosition | real | Vector | 3 | 3 | 3 | 5.0 | 211tel | - |  |
| 399 | SWMG_SetJumpSpeed | real | Void | 3 | 3 | 3 | 5.0 | 211tel | - |  |
| 400 | SWMG_SetPlayerInvincibility | real | Void | 3 | 3 | 3 | 5.0 | 211tel | - |  |
| 401 | SWMG_SetSpeedBlurEffect | real | Void | 6 | 3 | 3 | 5.0 | 211tel | - |  |
| 402 | SetGlobalLocation | real | Void | 5 | 5 | 4 | 5.0 | 403dxn | - |  |
| 403 | SoundObjectGetPitchVariance | missing | Float | 3 | 3 | 3 | 5.0 | 211tel | presentation | missing: not dispatched, returns 0.0 |
| 404 | SoundObjectSetFixedVariance | real | Void | 3 | 3 | 3 | 5.0 | 211tel | - |  |
| 405 | SoundObjectSetVolume | real | Void | 6 | 3 | 3 | 5.0 | 211tel | - |  |
| 406 | VectorNormalize | real | Vector | 5 | 3 | 3 | 5.0 | 211tel | - |  |
| 407 | AddMultiClass | real | Void | 11 | 2 | 1 | 4.0 | chitin | - |  |
| 408 | AdjustCreatureSkills | noop | Void | 3 | 2 | 1 | 4.0 | chitin | rules | no-op; skill adjust not applied |
| 409 | DisableMap | noop | Void | 4 | 4 | 1 | 4.0 | 305nar | hud | no-op; map disable not modelled |
| 410 | EffectForcePushed | partial | Engine(0) | 7 | 2 | 1 | 4.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 411 | EffectForceShield | inert | Engine(0) | 2 | 2 | 1 | 4.0 | chitin | rules | inert; force shield absorb not applied |
| 412 | EffectSavingThrowDecrease | real | Engine(0) | 7 | 2 | 1 | 4.0 | chitin | - |  |
| 413 | EffectTemporaryHitpoints | real | Engine(0) | 4 | 2 | 1 | 4.0 | chitin | - |  |
| 414 | EventSpellCastAt | real | Engine(1) | 86 | 2 | 1 | 4.0 | chitin | - |  |
| 415 | GetFacingFromLocation | real | Float | 4 | 4 | 1 | 4.0 | 403dxn | - |  |
| 416 | GetGameDifficulty | stub | Int | 2 | 1 | 1 | 4.0 | 001ebo | - | returns 1 (normal) fixed |
| 417 | GetIsLinkImmune | stub | Int | 6 | 2 | 1 | 4.0 | chitin | rules | FALSE; immunity links not modelled |
| 418 | GetItemHasItemProperty | stub | Int | 1 | 1 | 1 | 4.0 | 101per | items | FALSE; item property check not modelled |
| 419 | GetLastClosedBy | real | Object | 1 | 1 | 1 | 4.0 | 105per | - |  |
| 420 | GetLastWeaponUsed | stub | Object | 1 | 1 | 1 | 4.0 | 101per | rules | invalid; last weapon not tracked |
| 421 | GetSkillRankBase | real | Int | 4 | 2 | 1 | 4.0 | chitin | - |  |
| 422 | ModifyReflexSavingThrowBase | noop | Void | 2 | 2 | 1 | 4.0 | chitin | rules | no-op |
| 423 | RebuildPartyTable | noop | Void | 1 | 1 | 1 | 4.0 | 003ebo | scripts | no-op; party is a plain list here, so nothing to rebuild |
| 424 | ActionCastFakeSpellAtObject | real | Void | 4 | 3 | 3 | 3.0 | 506ond | - |  |
| 425 | DisplayDatapad | real | Void | 4 | 3 | 2 | 3.0 | 404dxn | - |  |
| 426 | EffectCutSceneStunned | inert | Engine(0) | 3 | 3 | 3 | 3.0 | 506ond | presentation | inert; cutscene visual |
| 427 | EffectModifyAttacks | real | Engine(0) | 13 | 2 | 2 | 3.0 | chitin | - |  |
| 428 | GetAlignmentGoodEvil | real | Int | 5 | 3 | 1 | 3.0 | 851nih | - |  |
| 429 | GetIsTrapped | real | Int | 6 | 2 | 2 | 3.0 | chitin | - |  |
| 430 | GetXP | real | Int | 3 | 3 | 1 | 3.0 | 711kor | - |  |
| 431 | SetMaxHitPoints | real | Void | 3 | 3 | 3 | 3.0 | 302nar | - |  |
| 432 | SetXP | real | Void | 3 | 3 | 1 | 3.0 | 711kor | - |  |
| 433 | SpawnMine | real | Void | 14 | 3 | 3 | 3.0 | 351nar | - |  |
| 434 | ActionRandomWalk | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 435 | AngleToVector | real | Vector | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 436 | CancelPostDialogCharacterSwitch | noop | Void | 1 | 1 | 1 | 2.0 | chitin | dialog | no-op; post-dialog switch cancel |
| 437 | DetonateMine | real | Void | 5 | 1 | 1 | 2.0 | chitin | - |  |
| 438 | DisplayFeedBackText | noop | Void | 9 | 1 | 1 | 2.0 | chitin | hud | no-op; feedback text not shown |
| 439 | DisplayMessageBox | real | Void | 4 | 2 | 2 | 2.0 | 650dan | - |  |
| 440 | DoDoorAction | real | Void | 4 | 1 | 1 | 2.0 | chitin | - |  |
| 441 | EffectACIncrease | real | Engine(0) | 24 | 1 | 1 | 2.0 | chitin | - |  |
| 442 | EffectBodyFuel | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 443 | EffectCrush | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 444 | EffectDamageDecrease | real | Engine(0) | 4 | 1 | 1 | 2.0 | chitin | - |  |
| 445 | EffectDamageForcePoints | real | Engine(0) | 6 | 1 | 1 | 2.0 | chitin | - |  |
| 446 | EffectDamageImmunityIncrease | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert; damage immunity not applied |
| 447 | EffectDamageResistance | real | Engine(0) | 38 | 1 | 1 | 2.0 | chitin | - |  |
| 448 | EffectDroidScramble | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 449 | EffectEntangle | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 450 | EffectFactionModifier | inert | Engine(0) | 3 | 1 | 1 | 2.0 | chitin | rules | inert; faction change via effect not applied |
| 451 | EffectForceBody | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 452 | EffectForceFizzle | inert | Engine(0) | 19 | 1 | 1 | 2.0 | chitin | presentation | inert; visual on force power fizzle |
| 453 | EffectForceResistanceIncrease | real | Engine(0) | 4 | 1 | 1 | 2.0 | chitin | - |  |
| 454 | EffectForceResisted | inert | Engine(0) | 6 | 1 | 1 | 2.0 | chitin | rules | inert |
| 455 | EffectForceSight | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 456 | EffectFury | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 457 | EffectImmunity | inert | Engine(0) | 7 | 1 | 1 | 2.0 | chitin | rules | inert; immunity not applied |
| 458 | EffectLightsaberThrow | inert | Engine(0) | 4 | 1 | 1 | 2.0 | chitin | rules | inert |
| 459 | EffectMindTrick | partial | Engine(0) | 2 | 1 | 1 | 2.0 | chitin | rules | debilitates via powers.rs DEBILITATING list |
| 460 | EffectPsychicStatic | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 461 | EffectRegenerate | inert | Engine(0) | 2 | 1 | 1 | 2.0 | chitin | rules | inert; regeneration not applied |
| 462 | EffectSavingThrowIncrease | real | Engine(0) | 33 | 1 | 1 | 2.0 | chitin | - |  |
| 463 | EffectSkillIncrease | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 464 | EffectTrueSeeing | inert | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | rules | inert |
| 465 | EffectVPRegenModifier | inert | Engine(0) | 3 | 1 | 1 | 2.0 | chitin | rules | inert |
| 466 | EffectWhirlWind | partial | Engine(0) | 1 | 1 | 1 | 2.0 | chitin | - | effect name read by k2-world (see powers.rs) |
| 467 | EndGame | real | Void | 2 | 2 | 2 | 2.0 | 411dxn | - |  |
| 468 | FaceObjectAwayFromObject | real | Void | 3 | 1 | 1 | 2.0 | chitin | - |  |
| 469 | GetAttemptedMovementTarget | stub | Object | 1 | 1 | 1 | 2.0 | chitin | pathing | invalid |
| 470 | GetBlockingCreature | stub | Object | 1 | 1 | 1 | 2.0 | chitin | pathing | invalid; blocked-by not tracked |
| 471 | GetBlockingDoor | stub | Object | 2 | 1 | 1 | 2.0 | chitin | pathing | invalid; blocked-by not tracked |
| 472 | GetCreatureMovmentType | stub | Int | 1 | 1 | 1 | 2.0 | chitin | - | returns 0 fixed |
| 473 | GetCreatureSize | stub | Int | 2 | 1 | 1 | 2.0 | chitin | - | returns 3 (medium) fixed |
| 474 | GetFactionEqual | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 475 | GetFoundEnemyCreature | stub | Object | 1 | 1 | 1 | 2.0 | chitin | rules | invalid |
| 476 | GetGlobalString | real | String | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 477 | GetIsDoorActionPossible | real | Int | 4 | 1 | 1 | 2.0 | chitin | - |  |
| 478 | GetIsPlayerMadeCharacter | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 479 | GetLastDamager | real | Object | 5 | 1 | 1 | 2.0 | chitin | - |  |
| 480 | GetLastForfeitViolation | stub | Int | 4 | 1 | 1 | 2.0 | chitin | minigames | FALSE |
| 481 | GetLastPerceptionVanished | real | Int | 2 | 1 | 1 | 2.0 | chitin | - |  |
| 482 | GetLastSpellCaster | real | Object | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 483 | GetLastSpellHarmful | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 484 | GetModuleFileName | real | String | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 485 | GetSelectedPlanet | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 486 | GetSpellForcePointCost | real | Int | 2 | 1 | 1 | 2.0 | chitin | - |  |
| 487 | GetSpellSaveDC | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 488 | GetSubRace | stub | Int | 4 | 1 | 1 | 2.0 | chitin | - | returns 0 fixed |
| 489 | GetTotalDamageDealt | real | Int | 2 | 1 | 1 | 2.0 | chitin | - |  |
| 490 | IsFormActive | stub | Int | 9 | 1 | 1 | 2.0 | chitin | rules | FALSE |
| 491 | IsMeditating | stub | Int | 2 | 1 | 1 | 2.0 | chitin | rules | FALSE |
| 492 | ModifyFortitudeSavingThrowBase | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 493 | MusicBattleStop | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 494 | PauseGame | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 495 | RemoveEffectByExactMatch | real | Void | 4 | 1 | 1 | 2.0 | chitin | - |  |
| 496 | ResistForce | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 497 | SWMG_GetEnemy | real | Object | 2 | 2 | 2 | 2.0 | 421dxn | - |  |
| 498 | SWMG_GetLastObstacleHit | real | Object | 2 | 2 | 2 | 2.0 | 371nar | - |  |
| 499 | SWMG_OnDamage | real | Void | 2 | 2 | 2 | 2.0 | 421dxn | - |  |
| 500 | SetCurrentForm | noop | Void | 1 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 501 | SetGlobalString | real | Void | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 502 | SetMapPinEnabled | noop | Void | 2 | 2 | 2 | 2.0 | 402dxn | - | accepted and ignored |
| 503 | SetReturnStrref | noop | Void | 5 | 1 | 1 | 2.0 | chitin | - | accepted and ignored |
| 504 | SoundObjectSetPosition | noop | Void | 1 | 1 | 1 | 2.0 | chitin | audio | no-op; sound object position not applied |
| 505 | SpeakString | real | Void | 14 | 1 | 1 | 2.0 | chitin | - |  |
| 506 | d12 | real | Int | 1 | 1 | 1 | 2.0 | chitin | - |  |
| 507 | ActionCastSpellAtObject | real | Void | 2 | 1 | 1 | 1.0 | 504ond | - |  |
| 508 | ActionSurrenderToEnemies | noop | Void | 2 | 1 | 1 | 1.0 | 403dxn | - | accepted and ignored |
| 509 | AmbientSoundPlay | noop | Void | 1 | 1 | 1 | 1.0 | 402dxn | audio | no-op; ambient sound not routed to audio |
| 510 | AmbientSoundStop | noop | Void | 1 | 1 | 1 | 1.0 | 402dxn | audio | no-op; ambient sound not routed to audio |
| 511 | DuplicateHeadAppearance | noop | Void | 1 | 1 | 1 | 1.0 | 711kor | presentation | no-op |
| 512 | GetInventoryDisturbItem | real | Object | 1 | 1 | 1 | 1.0 | 303nar | - |  |
| 513 | GetIsImmune | stub | Int | 1 | 1 | 1 | 1.0 | 506ond | rules | FALSE; immunity not modelled |
| 514 | GetMinOneHP | real | Int | 1 | 1 | 1 | 1.0 | 403dxn | - |  |
| 515 | SetInfluence | real | Void | 10 | 1 | 1 | 1.0 | 301nar | - |  |
| 516 | SetTrapDetectedBy | real | Int | 1 | 1 | 1 | 1.0 | 351nar | - |  |
| 517 | UnlockAllSongs | noop | Void | 1 | 1 | 1 | 1.0 | 904mal | - | no-op |
| 518 | YavinHackDoorClose | noop | Void | 1 | 1 | 1 | 1.0 | 303nar | minigames | no-op; minigame door |

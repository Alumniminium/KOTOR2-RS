# Area population (superseded by game.rs, see integration.md)

Objects are now spawned from k2-world objects by `game::sync_objects`, and
doors.rs is gone (doors open through world events). The notes below
describe the earlier GIT-driven version; the model, idle and door clip
findings still apply.

## Invisible, untargetable placeables (cutscenedummy pose)

Bug (101PER Security Desk, tag SecTer, plc_desk1): the desk was not drawn and
could not be hovered or used, although the world object was fine (using it
headless starts the `secter` conversation). Cause: `game::sync_objects`
treats a placeable whose `default` clip has a `cutscenedummy` track as an
area-space stunt pose (p_kreiastunt keys it at the area coordinates
(-21.99, 12.30, 9.11)) and pins the object root to the area origin. Ordinary
models such as plc_desk1, plc_chair2 and plc_datapad also carry a
`cutscenedummy` node in `default`, keyed at (0, 0, 0): identity, not a pose.
They were pinned to the origin too, far under the station, so they
vanished and their pick sphere (centered on the root) was nowhere near the
player. 142 placed objects were affected (105 chairs, 35 desks, 2
datapads).

Fix: only a `cutscenedummy` position key away from the origin (> 1 cm)
marks an area-space pose. Test: crates/k2-world/tests/useable_models.rs
(desk model facts, using the desk runs `secter` and its a_secterjmp, and a
survey of every placeable model in every module).

Survey of useable placeables without drawable geometry (all modules): only
plc_invis (68 useable instances, e.g. the 101PER Adm_Console, whose console
is baked into the room model), plc_inveb (19, event triggers in 002EBO)
and plc_scrysblu (3 in 604DAN, `cryform_bl`, a render=false box). Invisible
by authoring; they stay targetable through their pick sphere. Every useable
placeable's model resolves and loads.

## Original notes (crates/kotor2/src/area.rs, doors.rs, hover.rs)

## Spawning

- Module data comes from `k2_data::Module::load` in `load_module`; it is
  kept as the `AreaModule` resource, next to `GameData { tables, talk }`.
- Startup order: `load_module` (rooms, camera) > `area::populate` (GIT
  objects) > `play::setup` (walkmesh, player).
- Placeables: UTP Appearance > placeables.2da modelname. Doors: UTD
  GenericType > genericdoors.2da modelname. Creatures: k2-data
  `Creature::models` (appearance.2da, armor body variation, heads.2da) into
  `anim::spawn_creature`.
- Every object is placed with translate(X, Y, Z) * rotate_z(bearing).
  k2-data bearings are rotations about +Z for a +Y-facing model, which is
  how all KotOR models face at rest. IFO entry yaw (from +X) is
  `entry_bearing + PI/2`.
- Each root carries `GitRef { kind, index }` (GIT list and index) and
  `Tag`, for the world layer to map entities onto k2-world objects.
- Idle clip on spawn, first that exists: pause1 (humanoids), cpause1
  (simple creatures, e.g. droids use c-prefixed clips), default
  (placeables), closed (doors).
- Load times (release): 101PER 75 placeables, 22 doors, 18 creatures in
  ~100 ms; 301NAR ~140 ms; 501OND ~120 ms. Rooms 20-60 ms on top.

## Walkmesh additions

- Placeable `<model>.pwk` is added as an object mesh with the placeable's
  transform (101PER: 74 of 75 placeables have one).
- Door DWKs: `AreaDoors` is indexed by GIT Door List index (None for a door
  without model/walkmesh), so `GitRef.index` finds a door's handle.

## Targeting

- Only useable placeables, non-static doors and creatures get an
  `Interactable` (the original does not let you target chairs or the
  invisible helper placeables like `inv_talker`).
- Target = object whose pick sphere (around the root, raised by half its
  height) the cursor ray hits, else the nearest one within 4 m in front of
  the player. The name (TLK) is drawn above it.

## Doors

- Door models animate with opening1/closing1 (1.2 s one-shots) and hold
  opened1/closed (zero-length). Some also have opening2/opened2 for the
  other swing; not used yet.
- E or left click on the targeted door within 2.5 m (minus its radius)
  toggles it: plays the one-shot, then holds the end pose, and switches the
  DWK between Closed and Open1. Locked UTDs only log "locked".
- Door models contain an untextured `trans` mesh filling the doorway
  (transition marker, render flag set). The engine never draws it; the
  model loader skips any node named `trans`.

## Player

- Default appearance row 137 (P_MAL_A_MED_01): body pmbam, head pmha01,
  texture pmbama01. `--player-appearance ROW` picks another.
- appearance.2da walkdist/rundist are meters per walk/run cycle (pmbam:
  1.813 / 3.66 with walk/run clips ~1.03 s / ~0.67 s, i.e. ~1.75 / ~5.4
  m/s). Playback rate = ground speed * clip length / dist, so feet match the
  ground when sliding along walls too.

# Controls and third-person play (crates/kotor2/src/play.rs)

## Controls

- W/S forward/back along the camera yaw; A/D turn the character and camera
  together (2.4 rad/s). The character turns to face its movement direction
  (12 rad/s), or the camera heading when turning in place.
- Run 5.4 m/s by default; Shift toggles walk (1.75 m/s).
- Q/E rotate the camera (2.2 rad/s) without turning the character. Right or
  middle mouse drag orbits it (yaw, pitch -0.4..1.3 rad; the drag uses the
  cursor movement, falling back to raw device motion), mouse wheel zooms
  (1.5..12 m).
- The camera eases: the drawn yaw follows the wanted one (14/s), the focus
  follows the character (9/s), and while the leader walks (keys, click or a
  control walk) the camera swings behind it (6/s, `CAM_FOLLOW_RATE`). It stays
  put while the leader stands still, while you turn or orbit it (mouse drag,
  Q/E, A/D), and while you walk backward (S).
- F or left click: use the targeted object (see population.md). Out of reach,
  the leader walks to a spot within its use reach that a walkmesh route reaches
  (`World::begin_use_search` and `step_use_search`, nav.rs), round walls. An
  unlocked closed door on the route is opened and the walk waits for the swing;
  a door that does not open ends the walk. A locked door costs 40 m of route, so
  the route goes round it when it can; a locked door on the leg ends the walk
  once the leader is in reach of it, with the locked-door feedback. One search
  covers all candidate spots (`World::use_candidates`)
  and runs a frame at a time (the cost is measured in walkmesh.md, Budgets),
  until a spot is reached or the search gives up. The stand is fixed at click
  time: a moving target is not followed. A new click searches afresh. Holding W
  or S suspends the walk; the target stays and the walk resumes on release. A
  and D turn the
  character only when no walk is running: during a walk it faces the walk.
  When no such spot exists, or the walk makes no progress for 1.5 s, the walk
  ends and the game says "Unable to reach target!" (dialog.tlk 1516) if the
  leader is still out of reach. The spot is chosen with the game's own reach
  distance (object centre plus radius), not checked against the original's
  approach points.
- Quick action bar (bottom left, keys 1-4): on a door or container it offers
  Open/Close/Use (slot 0), Security or Unlock with a key (slot 1, needs a
  pickable lock) and Attack (slot 2, bashable only). The hover target stays
  while the cursor is over the HUD so the bar can be clicked.
  A non-hostile creature offers Talk (slot 0, when it has a conversation) or
  Use (a ScriptDialogue but no conversation); a corpse with loot offers Open.
  Each uses the same call as F (`World::use_object`). Icons: Use and Talk are
  the cursor art gui_mp_useu / gui_mp_talku (swpc_tex_gui.erf; cursors.2da
  names the Examine, Attack and Trap cursors too, but those textures are not
  in the install), Security is isk_security (the skills.2da icon), Attack is
  i_attack (tutorial.2da uses it too).
- F1 toggles the free-fly debug camera (WASD/QE, right mouse look). Starting
  with `--cam` or `--fly` begins in fly mode.
- Cutscenes hold the keys and click walks. While the party is held
  (`World::party_held`: a conversation, which a door's message box is too, a
  scripted input block, scripted leader actions, or a leader that cannot act)
  or a movie card is up, the keys do not move the leader, and a click walk in
  progress is dropped, not resumed.

## Setup at module load

- Area walkmesh = every LYT room's WOK (already world space) plus GIT door
  DWKs. Doors: GIT Door List X/Y/Z/Bearing/TemplateResRef; UTD GenericType
  indexes genericdoors.2da `modelname`; `<model>0/1/2.dwk` are mounted with
  translate(X,Y,Z) * rotate_z(Bearing) and start closed (Open1/Open2 if an
  OpenState field says so; vanilla GIT doors carry no OpenState). Handles
  now live in k2-world (`World::walk`); see integration.md.
- Player spawns at module.ifo Mod_Entry_X/Y/Z facing atan2(Dir_Y, Dir_X).
- Safe spawn: some entry points sit on walkmesh islands. 101PER's entry is on
  a 2-face, 3 m^2 Metal platform (z 9.05) under the Exile's bed, separated
  from the medbay floor (z 9.02) by a gap with no faces at all. The leader is
  moved to the nearest sampled point (rings of 0.25 m, up to 10 m) from which
  it can walk 2 m in at least two of eight directions. 101PER: moves 1.0 m.
  `WalkMesh::safe_spawn` (k2-walk lib.rs) does this; k2-world calls it on module
  entry and kotor2 game.rs at area load.

## Movement model

- The leader steps with `World::player_move` (`move_slide` at the leader's
  clearance, `World::leader_clearance`: its collision radius, at most 0.5 m, 0.3
  m with no creature), then living creatures block it. Z is snapped to
  `ground()` afterwards.
- `Player { pos, facing }` is game space; the player entity sits under
  WorldRoot so its Transform is game space too. The body is an animated
  creature child (see population.md), rotated -90 degrees about Z because
  models face +Y; a capsule is the fallback if the appearance fails.
- The world owns the leader's position. Each controlled frame `move_player`
  takes `Player.pos` and `facing` (bearing + 90 deg) from the world's leader,
  steps the player's own walk, and writes the result back with
  `World::set_player_position`. A walk the world made since the last frame (a
  control walk, a debug move) is where the frame starts.
- `PlayerMoveState { speed, moving, running, world_driven }` is set every frame
  by `move_player`. `moving` covers the player's own steps and a world walk.
  `world_driven` is Some(run) while the world walks the leader (run above 3.575
  m/s, the midpoint of walk and run speed) and None for the player's own input.
  `animate_creatures` (game.rs) fades the walk or run clip from it
  (`world_driven`, else `running`) while speed is above 0.2 m/s; otherwise the
  scripted or idle clip plays (ready stance in combat).

## Camera

- Follow cam looks at feet + 1.5 m from `dist` behind along (yaw, pitch).
- Collision: rendered geometry, see Follow camera collision (revised) below.
  The distance pulls in at once and eases back out (rate 6/s).

## Control walks (k2-world control.rs)

- A control walk (move_to, or a verb that walks into reach first) steers the
  leader as a click walk does. An unlocked closed door on its route is opened
  once the leader is in use reach, and the walk waits for the swing; that wait
  does not count toward the walk's no-progress clock. A door that opens into a
  conversation ("talks instead of opening") or stays shut ("did not open") ends
  the walk with an error.
- A locked door ends the walk with "<door> is locked". The use gives the
  locked-door feedback: OnFailToOpen fires, and the door's talk runs when it
  has one.
- `World::walk_held` is `party_held` without a door's message box, so a box
  raised mid-walk does not interrupt the walk. A cutscene or movie that starts
  mid-walk ends it with "interrupted: a cutscene started".

## Verification

    kotor2 --module 101per --autowalk SECS[,YAWDEG] --screenshot out.png --frames N

(`--autouse SECS` additionally uses the target at SECS) holds W for SECS with the camera turned YAWDEG from the entry direction and
logs position, speed and ground material once a second. Results:
- 101PER yaw 0: runs 5.4 m/s across the medbay and stops at the closed door
  PeragusDoor1 (-10.77, 6.78); ground Metal throughout.
- 101PER yaw 90: slides along the medbay wall and stops; yaw 180: blocked by
  the bed island edge.
- 301NAR: 5.4 m/s straight along the walkway, Metal, z constant 9.67.

## Since integration

F or left click uses the hover target, walking there first when it is out
of reach. Hostile creatures are attacked. Esc opens the pause menu, F3
toggles the FPS line, and in conversations 1-9 or a click picks a reply
while Space/Enter skips a line. Movement input breaks off an attack.

## Follow camera collision (revised)

- Collides with the rendered geometry, not the walkmesh: rooms and placeables
  are tagged (`RoomModel` -> `RoomMesh`) and ray-cast with Bevy's MeshRayCast
  (feature `mesh_picking`; meshes keep a main-world copy for this). Opaque
  materials only: the 101per leader starts inside a glass kolto tank. Backfaces
  count, for leaders standing inside a placeable's bounds.
- The walkmesh LOS test was dropped for the camera: its non-walk faces (bed and
  tank footprints) blocked views the render geometry leaves open and collapsed
  the camera into the head at the 101per start.
- Two rays per candidate (eye along the view, chest to the camera spot), so a
  console between camera and body counts. Blocked views first lift the pitch
  (up to +0.9 rad in 7 steps) to keep >= 1.5 m, else take the most open lift
  and pull in; the leader is hidden below 0.7 m.
- F1 / --cam fly camera now overrides dialog and cutscene cameras.
- Debug: K2_CAMDBG=1 logs the candidates each frame.

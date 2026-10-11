# Walkmesh notes (k2-walk)

Verified on the Aspyr K2 install. Tools: `cargo run --release -p k2-walk
--example {explore,mdlbox,wokspace,dwk}`.

## Room walkmeshes (WOK, BWM type 1)

- One WOK per LYT room, same resref as the room model.
- Vertices are already in world space; add them as-is. Do NOT add the LYT room
  position. Proof: for 1091 of 1108 rooms across all 82 modules, every WOK
  vertex equals the room MDL's AABB-node vertex + AABB node position + LYT
  room position (within 0.01). The 17 others match all but a handful of
  vertices (the WOK was re-exported after small edits).
- The BWM header `position` field equals the AABB node's position inside the
  room model (model space). It is informational; not used for placement.
- Room bounds do not overlap the LYT position in any obvious way on 101PER
  (e.g. room 101PERac has LYT (-2.54, 0.65) but its WOK spans x -21.9..-2.6,
  y -35.9..-18.4), because the AABB node is offset inside the model.
- Some rooms have empty WOKs (0 faces, e.g. 101PERzc, 101PER2b).
- Some rooms carry huge flat backdrop faces (101PERbl: 2 faces, 120 m wide, at
  z 0 while the floor is at z 9.05). Ground queries take the highest walkable
  face at or below z + STEP_UP, so these never win against the real floor.
- Neighbouring room WOKs do not share vertices exactly at seams, so room
  boundary edges are computed by probing 5 cm across each unshared edge (in
  25 cm pieces) for walkable ground at the same height.

## surfacemat.2da

Columns: label, walk, walkcheck, lineofsight, grass, sound, name. 31 rows.
Walkable: Dirt, Grass, Stone, Wood, Water, Carpet, Metal, Puddles, Swamp, Mud,
Leaves, BottomlessPit, Door, Trigger. Not walkable: NotDefined, Obscuring,
Nonwalk, Transparent, Lava, DeepWater, NonWalkGrass, CRAP rows.
`lineofsight` = 1 is read as "blocks line of sight" (Nonwalk, Obscuring and
all floors are 1; Transparent, Door, Trigger, NotDefined are 0).

## Placeable walkmeshes (PWK) and door walkmeshes (DWK), BWM type 0

- Object-local space. Place with the GIT instance transform:
  translate(X, Y, Z) * rotate_z(Bearing).
- No AABB tree (aabb count 0); few faces.
- PWK: flat footprint on z = 0, material 7 (Nonwalk). Many small props have
  empty PWKs (e.g. plc_backpack).
- DWK: three files per door model, `<model>0.dwk`, `<model>1.dwk`,
  `<model>2.dwk` (e.g. dor_dan010/011/012):
  - 0 = closed: a 12-face Nonwalk box spanning the doorway, width along local
    X (-2.5..2.6), thin in Y, z 0..3.1.
  - 1 and 2 = open in either direction: a 2-face vertical quad where the
    swung leaf sits. Hooks (relative_hook1/2) mark the two sides of the door
    (y +/-0.81).
- k2-walk treats non-walkable object faces as blockers: their edges, projected
  to XY, are blocking segments. `add_door` mounts all three and `set_door`
  enables exactly one.

## Behaviour and numbers (101PER unless noted)

- 2328 faces, 7035 blocking segment pieces.
- ground(): ~70-95 ns per query (release, 200k random points).
- move_slide() with a 0.1 m step: ~260-380 ns per call.
- Entry point (module.ifo Mod_Entry_X/Y/Z) has walkable ground in 72 of 73
  modules with a LYT. The exception is 371NAR (swoop race track, a minigame
  module), whose entry is not on its walkmesh.
- Fuzz: 20000 random moves each on 101per, 201tel, 301nar, 701kor and 003ebo
  never left walkable ground.

## Pathfinding (nav.rs)

- Layered grid, not a face graph: room WOK faces are big and placeable
  footprints sit inside them, so a face graph cannot route around a crate.
  The walkable room surface is sampled every 0.4 m (`NAV_CELL`); each cell
  gets one node per distinct ground height (heights within 0.35 m merge), so
  bridges over floors are separate nodes. Neighbours link 8-way when the
  height change is under 0.6 m, the edge midpoint is on ground, and no room
  boundary segment crosses the edge. Each edge also stores its least distance
  to a room wall (`edge_gap`, capped at R_MAX, 0.45 m, computed only near
  walls), and a search walks an edge only when that distance is at least the
  radius, so a
  chain keeps clear of walls along its edges, not just at its nodes.
- Each node stores its clearance to room boundaries (capped at 1 m) and its
  distance to every nearby object blocker mesh (PWK/DWK). A node is passable
  for radius r if clearance >= r and every enabled object mesh that blocks is
  >= r away (a closed door a route may pass does not block; see Door routes).
  Edges crossed by object segments list those meshes. So door open or
  close needs no rebuild; `add_object` resets the grid, which is rebuilt
  lazily (or eagerly via `prepare_nav`).
- Doorways: a closed door (`add_door_link`) records its centre, its passage
  direction (across its longest blocker segment) and its floor. `link_doorways`
  joins the floor either side of it: node pairs whose straight line crosses the
  door's blocker, among nodes within 1.5 m of the points 0.6, 0.9 and 1.2 m
  either side of the centre, at most 3 m apart. At most 2 links per door
  (`DOOR_LINKS`); if no line crosses, the nearest pair is linked. Doorway links
  join the components whatever the door's state. `same_region` ignores doors,
  so it can say yes across a static door that the route search refuses.
- Door routes (`sync_door_passable`, k2-world doors.rs, sets each closed door's
  `set_door_route`): a static door, or a transit door (`linked_to_module` set),
  is a blocker and no route enters it. Any other closed door is passable at a
  cost: 2 m unlocked, 40 m locked. Each edge or doorway link across an enabled
  door adds its cost; an open door costs nothing. The sync runs at module
  entry, on save load, on `set_locked`, and when a bash unlocks a door.
- `find_path_avoiding_locked` is the search for NPC routes (`steer` with no
  budget): a locked door is a blocker for it (`locked_blocks`), so the route
  goes round it or there is none. The leader's searches keep a locked door as
  the costed passage above. An NPC opens an unlocked door on its route in
  `move_towards` (tick.rs).
- Search: A* with an ALT heuristic (8 landmarks, farthest-point picked per
  big component, static Dijkstra distances), weight 1.2, small wall-hugging
  penalty. Straight-line heuristics expanded the whole 301NAR grid (57k
  nodes, 4 ms) on its winding walkways; ALT cut that to ~0.2 ms.
- Smoothing: farthest-visible string pulling (galloping plus binary search)
  with a grid line-of-walk test that keeps 0.1 m extra clearance. The line
  test rejects any hard blocker crossed between 0.2 m samples: a wall, a
  placeable, a static or transit door, or a locked door (`hard_blocks`). A
  closed door can be thinner than the samples. An unlocked closed door is not
  hard: `move_slide` still refuses steps through it while it is shut, so the
  walk opens it on the way and waits for the swing.
- Budgets: `find_path_within(a, b, r, max_expand)` is `find_path` with at most
  `max_expand` nodes expanded (`find_path` uses `FULL_BUDGET`, 400k). When the
  budget runs out, the search uses the first goal candidate it has already
  reached, and the route is flagged `cut`; only a cut with no candidate reached
  is refused (`RouteMiss::OverBudget`). `find_route_nodes_checked` (a helper for
  the k2-walk tests; the game routes with `SpotSearch`) returns the
  unsmoothed node chain (every node on the way, goal last) with that flag, or
  the reason when there is no route. It costs the search alone, since string
  pulling runs line checks along the whole chain.
- Click walks: one search per click, not one per spot. `World::begin_use_search`
  makes a `SpotSearch` (k2-walk) whose goals are every candidate spot's passable
  nodes within use reach, in the start's component. Click goals have exit cost
  0, so the goal of least route cost ends the search. `World::step_use_search`
  runs it one frame at a time: each frame expands up to `USE_EXPAND` (20k) and
  keeps the open list, and the whole search is capped at `FULL_BUDGET` (400k),
  so a click is at most 21 frames. `World::use_point` runs the same search to
  its end, at most 64 frames (`USE_STEP_CAP`).
  The stand is fixed at click time: a moving target is not followed.
- The search's heuristic is unweighted, so it stays admissible: the planar
  distance to the goals' box, and per landmark the gap between the node's
  landmark distance and the goals' range of landmark distances. Each search
  therefore ends at the goal of least route cost plus exit cost.
- Steering (`steer_leader`) repaths with the same kind of search: a `SpotSearch`
  toward the destination, one step of 20k a frame, with the leader standing
  still until its route is found. A repath drops the old route and starts a new
  search, and clears the failure timer. While the search runs the walk does not
  count as walking, so the stall clock does not run. No frame runs two search
  steps, and a steering search that uses up `FULL_BUDGET` fails. Its goals are
  the candidates near the destination, each with exit cost: the gap beyond 0.5 m
  times 1000, plus the gap. A goal 1 cm beyond 0.5 m therefore costs 10 m more
  than one within it, so the nearest reachable candidate wins unless its route
  is more than 10 m longer. (A far candidate once left the leader stopped 2.9 m
  short of a reachable destination.) NPC repaths (`steer` without a budget)
  still call `find_path_avoiding_locked`, one full search per repath, in one
  call.
- A mover waiting for a door to swing (`door_waiting`, set by `move_towards`)
  does not count toward a MoveTo's stuck clock (tick.rs), so the move resumes
  when the door is open. A door that does not open stalls the move, and the
  clock runs.
- Superseded, per-spot design: before the cut rule, 20k found fewer routes than
  50k: 20k found 184 of 201 on 201TEL and 374 of 399 on 506OND; 50k found 391 of
  399 on 506OND (review probe, 800 random points per module, pairs 3 m or more
  apart on the same region).
- Superseded, per-spot design: one-off timing under load, not a committed measurement: no committed test
  reproduces these numbers (the probe that took them was temporary and is
  removed). Method: release build; the first 40 placeables and doors of each
  module; starts 8, 12 and 20 m out in 8 directions; one 20k search timed per
  candidate spot. Mean / p95 / max in ms, taken before the cut and
  edge-clearance rules: 201TEL 1.30 / 2.25 /
  25.3, 506OND 1.03 / 3.17 / 89, 301NAR 0.81 / 2.29 / 4.6, 101PER 0.37 / 2.02 /
  2.6, 001EBO 0.14 / 0.28 / 0.64, 503OND 0.01 / 0.02 / 0.45, 207TEL 0.04 / 0.27 /
  0.57. The machine ran at load average 35 to 45, so the maxima include
  contention: three 201TEL outliers (17 to 57 ms) repeated at 1.2 to 1.6 ms, and
  the review run saw a 20 ms outlier on 101PER. No committed test reproduces
  these numbers.
- `same_region(a, b)` and every search share one candidate list
  (`Query::endpoints`): the start snapped within `STEP_UP + 0.4` and every goal
  candidate within 3 m of the target in the start's component. Candidates on
  other islands are dropped, so a goal whose nearest candidate is on another
  island still routes through the nearest candidate of the start's island.
- Start snaps to the nearest passable node within 1.2 m. Goal candidates are
  every passable node within 3 m of the target (same component). The search
  aims at the nearest candidate; if that one is unreachable (inside a
  placeable or a static or transit door footprint, which no edge enters; a
  door a route may pass is entered at its cost), the search has
  seen everything the start reaches, so the nearest reached candidate wins.
  With 24 start points 8 m out, 6 of 97 placeable and door origins on 101PER
  found no route when `object_targets_report` (k2-world, ignored test) last
  ran: five medical cylinders within 3 m of (0, -5) and SecTer at (46.0,
  -54.5), about 67 m from them. Enclosure not checked; re-run the test to
  refresh the count.
  Different static components (walkmesh islands, e.g. the 101PER entry under
  the bed) return None in ~4 us.
- Routes end on the nearest reachable spot when the target is not one
  (`Paths::at_end`): an unforced MoveTo finishes there instead of pushing at
  the blocked target until the 1.5 s stuck timer ends it. Forced moves still
  wait for their timeout and then place the mover at the target.
- Start points inside a placeable or static or transit door footprint, or in a
  dynamically cut-off pocket, find no route at all; movers standing there stop
  (not handled).
- Backdrop planes: 421DXN (turret minigame) has a 6 km walkable face. Such
  faces (> 2500 m^2) do not widen the grid, the grid is capped at 800 m per
  axis and 1.5M cells (421DXN gets none; moves there are straight lines).
  Faces spanning > 4096 cells of the 2 m lookup grid are kept in a separate
  list instead of being rasterized; this took the 421DXN walkmesh build from
  3.2 s to 2 ms.

Numbers (release, radius 0.3, one run of `cargo test --release -p k2-walk
-- --nocapture`, before the edge-clearance rule; not re-run):

    module   nodes    build   random pairs found   avg      worst
    101per    31k     45 ms   70/70                0.27 ms  1.79 ms
    201tel   108k     84 ms   32/32                1.06 ms  5.80 ms
    301nar    49k     48 ms   79/79                0.19 ms  2.45 ms
    701kor    57k     59 ms   75/75                0.25 ms  6.90 ms
    003ebo     6k      8 ms   79/79                0.06 ms  0.17 ms

Measuring loop (tests/nav.rs `random_paths_stay_on_ground`): build is
`prepare_nav()`. Per module, 160 random walkable points from a fixed seed are
taken in consecutive pairs; each pair on the same region is routed with
`find_path(a, b, 0.3)` and timed with `Instant`. avg is the mean over those
calls (including unfound ones), worst the maximum.

301NAR cross-area path (215 m straight-line): 0.36 ms median when measured (load
dependent), 19 waypoints
(tests/nav.rs `cross_area_path_speed`: the farthest same-region pair of 400
random points, `find_path` run 10 times, sorted, element 5 taken). Paths are
checked by walking every segment in 0.25 m steps with ground-following Z and
no crossing of a live blocker (`path_on_ground`).

Tests (crates/k2-walk/tests/nav.rs): straight lines blocked by a wall are
routed around on 101PER (150/150) and 003EBO (170/170); every path is
followed by `move_slide` at radius 0.3 and 0.5 with no stuck mover (359 paths
per radius: 101PER 129, 201TEL 80, 003EBO 150; 718 in all). Report-only: `object_targets_report` (k2-world) and `pth_report`.

## Module path data (.pth)

Every module ships a `.pth` (GFF): `Path_Points` (X, Y, First_Conection,
Conections) and `Path_Conections` (Destination). 101PER has 133 points, 201TEL
202, 003EBO 99; all points stand on walkable ground and all 870 stored
connections are routable with route/straight ratios near 1.0. It is a static
toolset graph (no doors, no placeables) and sparser than the grid, so it
cannot route around footprints. It is only used as a cross-check
(`pth_report`), not for routing.

## PWK/DWK header position

Placeable and door walkmeshes (type 0) store vertices local to the walkmesh
node; the header `position` is that node's offset in the model (absolute
hooks = relative hooks + position). `add_object` applies it before the
object transform. Open-door DWKs depend on it: dor_per011's leaf quad is at
x -2.42 (slid open); without the offset it sat mid-doorway and open doors
still blocked their center. Room WOKs (type 1) are area-space and must not
get it. Regression test: crates/k2-world/tests/doors.rs.

# Animation and skinning notes

Verified against the Aspyr K2 install with `kotor2 --viewer`.

## Supermodel chains

Animations live in supermodels, found by following the MDL header's
supermodel name until "NULL". The first model in the chain that has a name
wins. Examples:

- pmbam (player male body): S_Female02 > S_Female01 > S_Male02 > S_Male01.
  Yes, the male player body chains through the S_Female* models in K2.
- pfbam (player female body): S_Female03 > S_Female02 > ...
- Core clips: pause1, walk and run come from S_Male02 (230 animations) or
  S_Female03. Talking: tlknorm, talk. Death: die, dead. Locomotion variants:
  walkss/walkds/walkst/walkrf, runss/runds/runst/runrf, plus walkinj/runinj.
  Combat clips are coded (g8a1, c3a1, f4a1a, ...).

Tracks bind to the creature's nodes by name (case-insensitive). The
animation node's `node_number` is not needed when matching by name.

## Controller data

- Position keys are offsets from the animated node's rest position, scaled
  by the animated model's `animation_scale` (pmbam has 1.06). The first walk
  key for rootdummy is about (-0.04, -0.005, -0.003), while its rest
  position is (0, 0, 1.126).
- Orientation keys are (x, y, z, w). With raw column count 2 a key is one
  packed u32: x and y in 11 bits each, z in the top 10 bits, w implied.
- Bezier controllers (column flag 0x10) carry 3 points per key. Only the
  value is used.
- Node rest rotations in the geometry header are stored (w, x, y, z).

## Skinning

- The bonemap is indexed by pre-order node index of the model (root = 0). It
  holds the bone slot as f32 bits, with -1 meaning "not a bone". For example,
  in pmbam's Torso skin, node 13 (torsoUpr_g) is slot 0.
- qbone/tbone hold one entry per pre-order node: the transform from the skin
  node's space into that node's space. That is the inverse bind pose relative
  to the skin node, so the inverse bind matrix for a slot is
  translate(tbone) * rotate(qbone). qbone is (w, x, y, z).
- Per-vertex bone indices are floats referring to slots, with -1 for unused;
  weights are 4 floats.
- In Bevy the joints are the node entities of the same spawned tree, and the
  skinned mesh entity's own transform is ignored. Skinned meshes get
  NoFrustumCulling because their rest AABB is meaningless once animated.

## Heads

- appearance.2da `normalhead` indexes heads.2da, whose `head` column is the
  head model (e.g. 41 = PMHA01).
- The head model is spawned under the body's `headhook` node. Its header
  sets the animation root to `neck_g`, so only the neck_g subtree takes body
  animation tracks. The head's own torso_g/rootdummy copies are just the
  attachment chain and must not be animated, or the torso transform is
  applied twice.

## Textures

Body meshes already reference texa + "01" (pmbam uses pmbama01, where
appearance texa = PMBAMA). Variants swap the 2-digit suffix and are passed
as `CreatureSpec.texture`.

## API (crates/kotor2/src/anim)

- `spawn_creature(&mut commands, &mut loader, &CreatureSpec { body, head,
  texture }, parent, transform) -> Option<Entity>`. The returned root holds
  `AnimPlayer` and `AnimBindings` and starts in pause1.
- `AnimPlayer::play(name, looping, speed)` (idempotent while playing),
  `fade_to(name, looping, speed, secs)`, `finished()`, `current()`,
  `progress()`, `seek(t)`, and `set.names()` / `set.has(name)`.
- `AnimBindings::entity("headhook")` and similar look up attachment nodes,
  e.g. for weapons in rhand/lhand.

## Gaps

Danglymesh is static. Clip events (footsteps, hits) are parsed but not
dispatched. Bezier tangents are ignored. Emitters are not animated.
Self-illumination tracks (controller 100) are ignored. Texture and alpha
animation are covered in "Force cages and animated materials" below.

## Gotcha

`--screenshot` exits as soon as the target file exists, so reusing a path
from an earlier run silently keeps the old image. Use fresh file names.

## 001ebo medbay PC pose (verified)

c_medbaypc is created at InvisibleLocationMarker (44.06, 37.62, 2.41),
Bearing 3.129 (faces -Y), and plays ANIMATION_LOOPING_DEAD_PRONE (30) ->
"prone" (S_Male02; pfbam/S_Female03 inherit it). Measured pose (viewer,
K2_NODEDBG=1, model facing +Y): head_g y -1.05, feet y +0.25, rootdummy
(-0.02, -0.65). In the world that puts the head at y 38.67 by the headboard
(+Y end, tall faces at y 39.6) and the feet near 37.3 over the foot section;
the mattress (walkmesh material 9, z 2.41) spans y ~36.8-39.4. Top-down shot
confirmed it; the earlier "legs past the mattress" was dialog-camera
perspective.

## Force cages and animated materials

Force cages are placeables plc_fccage / plc_fccage2 (placeables.2da rows
ForceCage, ForceCage2; UTPs fccage003.., forcecageblk00n are the invisible
blockers). They appear in 101per (Atton cage), 202tel, 208tel, 222tel, 262tel,
301nar and 506ond. The models have no emitters; the whole effect is material
animation, which the renderer ignored:

- Their field sheets use PLC_FrcDist02 (TXI `proceduretype cycle`, numx 2,
  numy 2, fps 16, additive) and PLC_FrcDist01 (`proceduretype water`,
  additive). Both meshes also set the mesh `animate_uv` flag with a scroll
  direction (e.g. 0, 0.01 for the cycle sheets, 0.02, 0 for the water
  sheets).
- Clips `close` / `default` hold node alpha 0.35 (cycle sheets 1.0);
  `close2open` fades alpha to 0 over 1 s, `open2close` back, `open` holds 0.
  Alpha is controller 132 on mesh nodes (1 float per key). The `on`/`off`
  and `on2off`/`off2on` clips (ANIMATION 200/201) carry more alpha tracks.
  Cage state changes (world PlaceableChanged) already pick close2open /
  open2close, so the fade now shows.

What the renderer does now (all in the material shader, driven by Bevy's
`globals.time`, so no per-frame CPU work):

- TXI `proceduretype cycle` (reone agrees: frame advance at fps, a loop is
  numx*numy/fps seconds): the first version never ran at all, because the
  first TXI line of a TPC footer can carry stray bytes in front of its key
  (PLC_FrcDist02 begins with "proceduretype cycle"), so the parser missed it.
  `TexInfo.procedure` carries numx/numy/fps; the
  shader picks frame floor(t * fps) % (numx * numy), row-major from the top
  left, and samples with gradients of the unwrapped UVs (no mip seams).
- TXI `proceduretype water`: approximated by a sinusoidal UV warp, phase
  `t * speed * 2` rad (speed 4 = 8 rad/s) and amplitude
  `distortionamplitude * 0.1` UV (reone does not implement water; these two
  factors are tuned, not measured). The original builds a real ripple
  height field; this is a shimmer, not a simulation.
- Mesh `animate_uv`: UV scroll of `uv_direction_x/y * scroll scale` UV units
  per second (scale 60 by default, see live tuning; reone's
  kUvAnimationSpeed is 250 and looked too fast; the file stores 0.01 to
  0.02 for the cages) plus a small sinusoidal jitter (`uv_jitter`,
  `uv_jitter_speed`). The first version used direction * 1 per second, which
  is far too slow. Materials are
  cached per (texture, lightmap, scroll) so meshes with different scrolls do
  not share one.
- Node alpha: `anim::queue_alpha_nodes` gives each mesh node whose own clips
  fade below 1 private cloned materials (switched to alpha blending when
  they were opaque), and `animate_alpha` writes the sampled alpha
  (`AnimPlayer::sample_alpha`, crossfaded) into their tint alpha. Additive
  materials multiply it into the colour, so alpha 0 is invisible. Only the
  model's own clips decide which nodes qualify.

Survey: `K2_INSTALL=.. target/release/examples/animfx [PREFIX]` (crates/k2probe/examples/animfx.rs) lists models with alpha / self-illumination
tracks, `animate_uv` meshes, emitter nodes and procedural-TXI textures.
Chitin placeables: 14 with cycle or water textures (cages, holomaps
plc_holo*, plc_swpmon*, plc_pgen), 30 with alpha clips (console
panels, rings, wallpnl, rakatcom, starmap), 3 with mesh scroll (both cages,
starmap). The TXI procedure kinds on chitin models are only `cycle` and
`water`. Room and effect models (101per2b, 221telc, 222tel06, 262telp,
001ebo16 ...) use the same mechanisms; rooms get the texture and scroll
animation through the same material path, but room alpha clips are not
played because rooms have no AnimPlayer.

Timing check without screenshots: `K2_UVDBG=1 kotor2 ...` logs one `uvdbg`
line per animated material at t = 0, 0.07, 0.13, 1, 2 s (cycle frame index,
scroll offset, water phase), computed by `KParams::describe_at`, which mirrors
the shader. PLC_FrcDist02: 16 fps, 4 frames (0 at 0 s, 1 at 0.07 s, 2 at
0.13 s), scroll (0, 2.5) uv/s. PLC_FrcDist01: scroll (5, 0) uv/s, water phase
8 rad/s. Unit tests: `cargo test -p kotor2 material model`.

`K2_UVDBG=1` also prints one `uvnode` line per mesh node at model load
(texture, blend, flags, animate_uv, direction, scroll, cycle, water, position,
bbox). plc_fccage2 data: two shell groups of four flat quads each around a
central column, every node with its own material params. Cycle group
(Object134, Cylinder332, Object135, Object133; PLC_FrcDist02, additive):
16 fps 2x2, scroll (0, 2.5) uv/s, quads about 0.58 m from the axis. Water
group (Mesh44, Mesh45, Object692, Mesh42; PLC_FrcDist01, additive): water
amp 0.2 speed 4, scroll (5, 0) uv/s, quads about 0.55 m from the axis.
plc_fccage: two cylinders (Cylinder33 cycle at radius 0.70, Object69 water at
0.665). The file gives all nodes of a group identical rates, so the parallax
comes from the two groups moving in different directions at different radii
(a 3 cm gap), not from per-node rates. Nodes with animate_uv 0 carry garbage
directions (Object140 0.33, -0.08) which are ignored. Plain meshes with the
same texture and scroll share one material, which is safe because the shader
is stateless.

Live tuning (`material::UvTuning`, rates feel-tuned by the user, not
measured against the original): scroll = file uv direction * scroll scale
(default 60, so cage sheets run 0.6 to 1.2 UV/s; reone uses 250, which was
too fast), cycle fps = TXI fps * cycle scale (default 0.5, so PLC_FrcDist02
runs 8 fps), water phase = TXI speed * 2 rad/s * water scale (default 1).
Keys (Numpad only; F5/F9 are quick save/load, F3 the debug line, and no
other binding uses the numpad): Numpad 1 / 2 scroll slower / faster,
Numpad 4 / 5 cycle, Numpad 7 / 8 water, each step x0.8 / x1.25. Every press
logs and shows a floating line, e.g. "uv scroll x60.0  cycle x0.50  water
x1.00", for 2.5 s. Env overrides: K2_UV_SCALE, K2_CYCLE_SCALE,
K2_WATER_SCALE. The values live in KParams.tune (a system copies changes
into animated materials), so the shader does the scaling. The tuning is
global, not saved.

Smoothness (rates unchanged):
- Cycle frames cross-fade: the shader samples frame i and i+1 and mixes by
  the fractional frame (`describe_at` prints it as "toward next"), instead of
  stepping at 8 fps. Each sample stays inside its own cell, inset by half a
  texel of the current mip level, so linear filtering never reads the
  neighbouring cell.
- PLC_FrcDist01 (128x128) and PLC_FrcDist02 (256x256, 2x2 cells) ship with a
  single mip level and no alpha channel (RGB, additive by TXI flag). Without
  mips the glitter aliased and shimmered as it scrolled, which looked rough.
  `model::build_mips` now builds a box-filtered chain for procedural
  textures that have one level; for cycle sheets it stops at the cell size so
  levels never mix cells.
- The water warp has two octaves, evaluated per pixel from a continuous
  phase; there is no quantisation in it.
- Additive materials use Bevy AlphaMode::Add: transparent pass, no depth
  write, and the blend is commutative, so shell draw order does not matter
  and coincident shells cannot z-fight (the cage shells sit at distinct
  radii anyway: 0.55 and 0.58 m, or 0.665 and 0.70 m).
- Alpha fades use the clip's linear keys (close2open 0.35 to 0 over 1 s) and
  are applied every frame; opening a cage plays close2open then holds open.
  The textures have no alpha channel, so no edge fade is implied by them.
- Time is Bevy's `globals.time` (f32 seconds, wrapped hourly): precision at
  3600 s is about 0.4 ms, and scroll uses fract(rate * t). It is smooth; the
  only discontinuity is the hourly wrap.
- The GPU shader was validated with naga (wgsl front end plus validator,
  uniformity included) after the change; no screenshot of the new shader
  exists because kotor2 windows open on workspace 4 and a hidden workspace
  gets no frames.

Still missing: emitters (cage lock sparks in plc_cage/plc_cagelg, console
sparks), self-illumination colour animation, light-node colour clips
(AuroraLight in the cage models), and the real water procedure.

Verified with 262tel (cages at 0.96,76.7,21.4): `--cam 0.96,71.5,23.2,90,-12
--frames 90/97/104` (same window size each run; the window size races under
load, so compare only same-size shots): before, mean crop difference
between frames is 0.0-0.014 (static); after, 0.4-1.3 (glitter moves). Viewer: `--viewer plc_fccage2
--anim open` is empty, `--anim close2open --atime 0.5` is half faded.

## Death clips and corpses

Clip names by model family (animations.2da rows die, dead, die1, dead1,
cdie, cdead, die3, dead3): humanoids (S_Male02/S_Female03 chains) have die,
die1, die3 and the loops dead, dead1, dead3; droids, rancors, kath hounds and
other monster models have cdie/cdead; c_dewback has no death clip at all.
The loop that matches the one-shot is the corpse pose: die > dead, die1 >
dead1, die3 > dead3, cdie > cdead. Do not pair across families (the droid
cdie/cdead tracks only drive the nodes of its own model).

Engine flow (crates/kotor2/src/game.rs):

- k2-world `kill()` emits `Died` and then `PlayAnimation 26` (the dead
  loop). The client must not let that second event replace the death clip:
  `apply_events` ignores animation 26 for a creature that is already dead, and
  the `Died` handler plays die/die1 (alternating by object id) or cdie.
- `animate_creatures` handles dead creatures first: wait for the die clip to
  finish, then loop its matching dead clip (or hold the last frame when the
  model has none). A creature that is dead with no death event (loaded from a
  save, spawned dead) goes straight to dead/dead1/cdead. A scripted `prone`
  pose on a corpse is left alone.
- `corpse_targets` keeps the Interactable marker on a corpse while it holds
  loot (`World::is_container`) and removes it otherwise; resurrection restores
  it. Previously every corpse lost it, so nothing could be looted.

Test hooks (screenshots, fresh PATH each run, run with `--fly`):
`K2_KILL=TEMPLATE[,FRAMES[,X,Y,Z]]` spawns and kills a creature; the area
must be player-controlled (no opening dialog). `K2_DEATH_SHOT=PATH[,FRAMES]`
points the fly camera at the first victim and shoots FRAMES frames after the
death; `K2_DEATH_CAM=DEGREES,METRES` picks the camera side and distance.
Example in 101per hangar floor: `K2_KILL=g_sithtroop01,60,-8.7,-20.4,9.05`.

## Sit-downs (ANIMATION_LOOPING_SIT_*, 36-40)

Rootdummy keys from the supermodel clips (animdump). Rest heights: male 1.126 m
(S_Male02), female 1.063 m (S_Female02). Rootdummy-only numbers; the seat
height is not measured against the pelvis.

- `sitdown` (S_Male02; S_Male01 has no sit clips): 1.03 s, rootdummy from
  (0, 0, 0) to (0.02, -0.46, -0.42). It ends 0.70 m high and 0.46 m back.
- `sit` (S_Female02): 2.0 s, ends at (0, -0.11, -0.84), rootdummy 0.22 m high.
  A floor-level pose; `meditatesit` holds the same end pose. `sitstand` rises
  from it in 1.67 s.
- Female bodies inherit `sitdown`: S_Female03 > S_Female02 > S_Female01 >
  S_Male02. So 36-40 map to `sitdown` only, and female sitters use the same
  chair-height sit. Their own `sit` is not used.

Neither sit clip returns to its start pose, so looping one snaps back up and
sinks again every clip length. The old client looped `sit` (game.rs anim_clips
36-40, `looping` from the world event). That is the likeliest cause of the
"sitting down on the floor in a loop" report; the in-game look still needs the
user to confirm. Now 36-40 play once and hold the last frame until the script's
duration runs out, or for ever with no duration. `scripted_playback` (game.rs)
holds the policy, and `animate_creatures` holds a one-shot until its deadline.
Other animations keep their flags. A non-looping animation ignores its duration
(nwscript: fire-and-forget animations do not use fDurationSeconds).

Seats:

- plc_chair2 (cutscenedummy, lookathook, one Box32 mesh) and plc_chair4 (one
  mesh, no hook nodes) have no sit or seat hook node.
- Room geometry supplies the seat. For 20 of 21 sitters checked (503ond 12 of
  12, 207tel 6 of 6, 201tel 2 of 3), an upward room face (normal z > 0.9)
  lies within 0.3 m (XY) of the point 0.46 m behind the GIT point, along
  -facing, where sitdown moves the root. Its height is 0.46-0.51 m above the
  GIT z. Lateral points have no such face. Front points have faces at
  0.71-0.77 m, which are not seats. Room MDL triangles were placed with the
  LYT room position, node chain and rotation.
- Placement is not changed: sitters stay on their GIT point, as the designers
  placed them.

Scripts:

- k_bar_spawn, k_bar_spawn2, k_bar_spawn3 play 37 (looping, duration -1) from
  the spawn, with no move. k_bar_spawnp and k_bar_spawnp2 play 38 after a
  conditional ActionForceMoveToObject to a waypoint. The 30 s runs took no
  move in 503ond or 207tel. The tag the guard builds was not decoded.
- 201tel traveller sitters play 36 (k_201comsit_sp), in place.
- 003ebo: anim 40 is in k_003ebo_enter, behind global 003_cutscene_mode == 0.
  No sitter was seen in 60 s, so it is not covered by a test.
- Forced moves to waypoints (k_def_spawn01, k_citz_sp, k_207_commoner) end on
  the waypoint.

Test: crates/k2-world/tests/sitting.rs (503ond, 207tel and 201tel sitters: one
sit each in 30 s, no move, no turn). Client policy: game.rs `sit_tests`.

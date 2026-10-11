# Minigames: swoop racing, turrets, pazaak

Code: `crates/k2-world/src/minigame.rs` (simulation plus the SWMG_* routines),
`crates/k2-world/src/pazaak.rs` (rules, AI, PlayPazaak/GetLastPazaakResult),
`crates/kotor2/src/minigame/` (view, input, camera, overlays, pazaak screens).
Headless harness: `k2world minigame MODULE [--seconds N] [--trace]` and
`k2world pazaak MODULE SCRIPT [--wager N] [--param N] [--trace]`.

## Where K2 uses them

Every minigame area has an ARE `MiniGame` struct (Type 1 = swoop, 2 =
turret). The scripts that send the party there use StartNewModule. The
SWMG_* calls in the shipped scripts (ranked by `cargo run -p k2-script
--example rscan -- swmg`) are all in these modules:

- 211tel, 371nar, 510ond: swoop tracks (Telos, Nar Shaddaa, Onderon).
- 421dxn: the Ebon Hawk dorsal turret over Dxun/Onderon.
- 505ond: the Onderon turret.

Pazaak: PlayPazaak is called from 003ebo, 202tel, 207tel, 301nar and
others. pazaakdecks.2da gives opponent decks (`+N`, `-N`, `*N` = +/-N, `VV`
= +/-1 or 2, `$$` double, `TT` tie breaker, `F1` flip 2&4, `F2` flip 3&6).
The player's cards are the items g_i_pazcard_001..023 (1-6 = +1..+6, 7-12 =
-1..-6, 13-18 = +/-1..6, 19 = +/-1/2, 20 = double, 21 = tie breaker, 22/23 =
flips).

## The MiniGame data

- Player and Enemies are followers: `Track` (a model in the LYT `tracks`
  section), Models (each with RotatingModel), Sphere_Radius, Hit_Points,
  Max_HPs, Invince_Period, Bump_Damage, death/engine Sounds, Scripts
  (OnCreate, OnHeartbeat, OnDamage, OnDeath, OnFire, OnHitBullet,
  OnHitFollower, OnHitObstacle, OnTrackLoop, OnAccelerate, OnBrake, ...), and
  Gun_Banks (Bullet model/damage/speed/lifespan/rate, Target_Type 1 = player,
  2 = enemies; enemies add Inaccuracy, Sensing_Radius and spreads). Enemies
  with `Trigger` (accelerator pads) aren't shot and don't block.
- Player-only fields: Camera (a model with a `camerahook` node),
  CameraRotate, Minimum/Maximum_Speed, Accel_Secs, Start_Offset, Tunnel*
  limits (TunnelInfinite marks a wrapping axis), Target_Offset.
- Mouse AxisX/AxisY map mouse X/Y to offset axes 1..3 (421dxn: X to 3 =
  yaw, Y to 1 = pitch).
- Obstacles: names of LYT `obstacles` models. Every obstacle model has one
  AABB walkmesh node `hitbump` and (almost) nothing drawn at it: the mgc
  crates and pillars are collision proxies for scenery drawn in the area's
  room models (checked: a drawn face sits on every reachable proxy box).
  The mge (event props) and mgf (force fields) obstacles are different:
  their only script is OnHitFollower, and the collision box is a trigger
  volume. mge01-04 (371nar), mge01-06 (211tel) and mge01-03 (510ond) run
  spec_obst_hit, which plays the model's `hit` animation (a freighter lifts
  off, the finish gate shutters close); the drawn mesh sits tens to hundreds
  of units from the box (371nar_mge01: box at y 2996 across the whole
  track, the freighter at y 3650 below it), so they were invisible walls.
  371nar_mgfd1/mgfu1/mgfl1/mgfl2/mgfr1/mgfr2/mgfr3 (the last stretch) run
  vertical_neg/pos, lateral_neg/pos: SWMG_PlayerApplyForce 30.
  `World::audit_minigame` / `k2world minigame MODULE --audit` list the
  boxes and what is drawn behind them (`K2_MDL_TREE=a,b` also dumps model
  trees and animations).

## Tracks

A track model has one node `modelhook` animated by an animation named
`track`. Its position keys are offsets from the hook's rest position (true
for every animated KotOR node: the 421dxn fighter hook rests at (35, 339,
14) and the keys move it hundreds of units around that); orientation keys
are absolute. Swoop player tracks run along +Y (371nar: 5092 units over a
96 s animation). The player moves by distance (speed units/s) and the track
time comes from the arc length; enemies just play their track (Num_Loops,
-1 = forever).

## Simulation (k2-world)

- Swoop: speed += AccelerationPerSecond (scripts set it per gear), clamped
  to the script min/max. The scripts treat the accelerate key as a gear
  shift (OnAccelerate sets min/max speed and acceleration for the next gear
  from the MIN_RACE_GEAR global: -5..-1 start countdown, 0 go, 1..5 gears;
  pressing it during the countdown is a false start and plays Penalty).
  Shifting is manual in play: Up / W (or left click) runs OnAccelerate, and
  a shift before the gear's top speed is the script's own penalty. Gear 0 is
  "go" (press once to start), gear 1 tops out at 70, gear 2 at 110, gear 3
  at 155 (the script's table 35/60/100/150/210 plus 10 or 5), checked by
  crates/k2-world/tests/swoop_gears.rs. The harness and `--mg-auto` set
  `World::auto_gears`, and `auto_shift` then presses the key for them when
  the gear is 0 or the speed reaches the gear's max speed, at most every
  0.35 s, and stops once a shift above gear 1 leaves the max speed
  unchanged (top gear; a crash drops the gear and the climb restarts).
  SWMG_GetSwoopUpgrade returns 0, so the upgrade multipliers in the script
  (+20% speed per level) never apply. See "Swoop upgrades" below.
- Steering: the scripts set the lateral acceleration to the current speed
  (0 until "Go", so there is no steering before it) and the old code turned
  that into a lateral speed of a tenth of it, 5 units/s in first gear.
  Now input accelerates the sideways speed at the script value clamped to
  120..300 units/s^2 (twice that when reversing) up to a peak of tunnel
  width / 1.1 s (20..38 units/s) and releasing it damps the speed
  (exp(-9 t)); hitting the tunnel edge zeroes it. Measured with
  `k2world minigame MODULE --probe`: a crossing of the drivable lane takes
  1.2 to 1.3 s in all three tracks, at the start and at 200 u/s.
  SWMG_SetJumpSpeed starts a jump
  (gravity 45 u/s^2, chosen so the script's ~30 u/s jump clears the 6-unit
  barriers); offsets clamp to the tunnel. Touching a follower runs its
  OnHitFollower (self = follower) and the player's; touching an obstacle
  runs OnHitObstacle and bumps (DoBumping). Obstacle collision uses the
  model's AABB walkmesh nodes when present, else its rendered triangles,
  keeping only wall-like faces (|normal.z| < 0.7). Obstacles with an
  OnHitFollower script are triggers: that script runs once on entry (self =
  the obstacle); no damage, no bump, no player OnHitObstacle, and the
  autopilot ignores them. The swoop rides on floor
  faces, and the big set pieces have bounding boxes that cover the whole
  track.
- Turret: the offset is the gun's rotation in degrees (x pitch inside the
  tunnel limits, z yaw wrapping). Gun banks fire while fire is held at
  Rate_Of_Fire; bullets are segment-vs-sphere tested; damage runs OnDamage
  (or the default damage animation) and, at 0 HP, OnDeath (or SWMG_OnDeath's
  default: die animation plus death sound). Enemies fire at the player
  inside their sensing radius.
- The player's OnHeartbeat runs every 0.1 s. Race timing reads the game
  clock there, so GetTimeMinute/Second/Millisecond now follow Aurora
  semantics: one game hour lasts MinPerHour real minutes, and minutes,
  seconds and milliseconds are real time within it (previously they were
  scaled 30x and lap times came out in game minutes).
- Saving is refused in minigame modules and during pazaak.

All SWMG_* routines in nwscript.nss are handled. SWMG_GetSound*/SetSound*
are accepted and ignored (the shipped scripts never call them);
SWMG_GetSwoopUpgrade returns 0 (no swoop upgrade state is tracked yet).

## Swoop upgrades (SWMG_GetSwoopUpgrade)

- swoopupgrade.2da: 13 rows. Row 0 is the none entry; groups 0, 1, 2 are
  engine (SwoopEngine01-04, desc strref 48570 "Engine Level:"),
  acceleration (SwoopAccel01-04, 48569 "Acceleration Level:") and frame
  (SwoopFrame01-04, 48571 "Frame Level:"), levels 1-4. The templates
  (s_e_rlift, s_a_sorosub, s_f_reinforce, ...) are not UTIs in the install.
- The 211tel scripts onaccelerate and onbrake call the routine with 0 and 1
  (engine and acceleration by the group column). The routine has 34 call
  sites in 18 scripts (`rscan SWMG_GetSwoopUpgrade`), among them 211tel mine,
  onaccelerate, onbrake, onfire, onjump and 371nar enemyhitfollower, hitvehicle,
  mine.
- Storage: not found. The purchase is engine-side: k_ptar_swoopup (chitin,
  and a copy in 003ebo) is only ShowSwoopUpgradeScreen() (ACTION 785), with
  no SetGlobal*, AddItem or CreateItem around it. No script names a swoop
  upgrade global: the chitin .nss sources (638 files) and the compiled
  module scripts have no such CONSTS string (the Engine01-05 strings are
  sound object tags). The swoopupgrade.2da templates (s_e_rlift, s_a_sorosub,
  s_f_reinforce, ...) are not UTIs in chitin or any module, and no script
  mentions them. The 6 saves under the install's cloudsaves/ hold only race
  times and champion flags, no swoop part items and no upgrade globals, so
  none of them shows a purchase. The g_i_upgrade001-009 UTIs are workbench
  weapon and armour upgrades (dialog.tlk 32487-32495), not swoop parts. The
  purchase screen says "Your Swoop Bike Currently has the following stuff on
  it." (dialog.tlk 48572). SWMG_GetSwoopUpgrade stays 0. Next step: buy one
  swoop upgrade in a save and diff that save's globalvars.res and savegame.sav
  against one from before the purchase.

## Sounds

The swoop sounds are GIT sound objects (UTS, Positional=0, Active=0 until a
script starts them) driven by the module scripts through SoundObjectPlay,
SoundObjectFadeAndStop, SoundObjectSetVolume and SoundObjectSetFixedVariance,
which kotor2/src/audio.rs `drive_sound_objects` now applies (World emits
`WorldEvent::SoundObject`, `SoundObjectVolume`, `SoundObjectVariance`):

- Countdown and start: PowerUp and Idle (loop) on creation, S1 twice, Go.
- Gears: onaccelerate stops the old Engine0N loop with a 0.5 s fade and
  starts Shift<N> plus Engine0N (mgs_engine_01..05, mgs_shift_01..05). The
  heartbeat sets the live Engine variance every 0.1 s; the values (1.2..4
  rising within a gear) are semitones: each gear's loop is about 2.7
  semitones above the previous one and the script revs each loop up to 4
  above itself, so the pitch is continuous across shifts. We play at
  2^(v/12) times speed.
- Crashes: Damage (mgs_bike_dmg) from the obstacle, mine and rival-bike
  scripts; Penalty (gui_error) for a false start or a missed gear; PowerDown
  at the end.
- Pickups: the ARE Sounds/Death of the accelerator pad followers
  (mgs_accelpad) plays, spatially, when the swoop touches the pad.
- A "Wind" sound object is referenced (SoundObjectPlay, volume speed/200)
  but none of the three tracks defines it, so it is silent.
- Turret modules: each enemy loops its ARE Engine sound (mgs_engine_01),
  spatial and following it, until it dies; Fire_Sound, Collision_Sound and
  Death sounds play at the event position (flat when it is the player's own).
  Falloff for follower sounds is linear 15..300 units.
- Every sound start logs at info: `audio: sound object NAME starts CLIP`,
  `minigame: sound CLIP at (x, y, z) (flat|spatial)`, `minigame: engine loop`
  and `audio: sound object NAME pitch xN` (on a 10% change).
  `k2world minigame MODULE` prints the tally of sound object calls.

## Results (headless, autopilot)

- 421dxn: 6/6 fighters destroyed, scripts count `421DXN_Dead_Enemies` and
  end the sequence (StartNewModule 003ebo).
- 505ond: 8/8, back to 504ond.
- 371nar: race finished with the autopilot shifting (`auto_gears`),
  back to 301nar after about 49 s.
- 510ond: finished, back to 503ond after about 48 s.
- 211tel: finished, back to 207tel after about 63 s.
- Pazaak, 202tel Mebla (k_paz_mebla param 2, 40-credit max wager): match
  played to the end, k_pazend_mebla ran with GetLastPazaakResult, credits
  moved by the wager. 207tel a_play_pazak (no wager) likewise.
- `k2world all --seconds 60`: 82 modules, 0 panics, 0 VM errors.

## Game side (kotor2)

- Every follower, obstacle and bullet is drawn from its models; the leader
  and the HUD are hidden; the camera sits on the camera model's
  `camerahook` (Bevy's -Z forward / +Y up match the hook's axes). The
  camera override runs after transform propagation but before frustum
  update; otherwise culling uses the follow camera's frustum.
- SWMG_PlayAnimation with bOverlay layers a clip on top of the main one (the
  race timer digits, gear lights, the turret's HUD rotation and health dial
  are all overlays); a new overlay replaces any overlay driving the same
  nodes; SWMG_RemoveAnimation removes it.
- Controls: swoop A/D or arrows steer, S/Down/Space or right click jump
  (Up / W or left click shifts up; the race timer digits are overlay clips
  with 92-unit position keys, so the junk-key filter in anim/clip.rs skips
  zero-length clips); turret mouse or arrows aim, left mouse or Space fires. Space does
  not pause the game inside a minigame (the pause toggle in gui/menus.rs
  checks `minigame.is_none()`); Esc still opens the options menu, which
  pauses.
- `--mg-auto` is a crude autopilot for screenshots (the turret aims and
  fires; the swoop needs nothing, it just drives straight).
- Pazaak: wager (pazaakwager), side deck (pazaaksetup), table (pazaakgame)
  from the game's layouts and card textures (pcards_pos/neg/dblpos/dblneg/
  gold/generic/back). Flip and change buttons choose the sign and value of
  +/- and 1/2 cards. The screens count as an open menu (the world pauses);
  Esc or Forfeit loses. Test hooks: `K2_MG_SCRIPT=name[:param]`,
  `K2_PAZAAK_AUTO=1`.
- Debug switch: `K2_NO_MINIGAME=1` runs minigame modules as plain areas.

## Effects: swoop shield, fighter explosions

- The swoop crash script (enemyhitfollower) plays `damage` as an OVERLAY
  clip. v_supertrike01's `damage` clip flickers the `k2_shield` mesh alpha
  (0.9 down to 0 over 0.97 s; the gear clips hold it at 0) and v_damageman's
  emitters. Overlays used to pose nodes only, so the shield never showed:
  overlay alpha now reaches the node materials (`anim::AlphaOverlay`), a
  running `damage` overlay is not replaced by the gear shift that follows a
  crash and wins over the gear clips while it plays.
- Fighters have emitter nodes (`crates/kotor2/src/emitter.rs`, doc comment
  has the controller details). Their `die` clip carries 11 `detonate` events
  at 0.07 s: Explosion emitters burst (Explode: one 45 m fireball from the
  4x4 sheet Fx_Explode_01 at 16 fps; Spark02: 100 flare sparks; Parts_01-09:
  Fx_part_01 debris chunks) and Smoke (Fountain) is driven 0 to 15 per second
  by the clip's birthrate track. Only minigame models run emitters
  (`FxLive`); thruster fountains with a constant birthrate are not drawn.
- Test hooks: `K2_MG_CRASH=SECS` (world) runs the player's OnHitObstacle once
  at SECS of minigame time; `K2_MG_SHOT=PATH,SECS,...` (with
  K2_TEST_WINDOW=1) screenshots at those minigame times. Example:
  `K2_MG_CRASH=9 K2_MG_SHOT='/tmp/x/c_{}.png,9.2,9.45' kotor2 --module 211tel
  --mg-auto`; turret: `--module 421dxn --mg-auto` with shots at 0.8 and 1.0.

## Fixed along the way (renderer)

TXI `blending additive` surfaces were drawn opaque: Bevy's AlphaMode::Add
is premultiplied blending, which expects alpha 0 from the shader, and our
KMaterial shader returned alpha 1. The turret cockpit glass (MGF_glass01,
additive with an env map) covered the whole view. The shader now
premultiplies and writes alpha 0 for additive materials (FLAG_ADDITIVE).

## Gaps

- Swoop upgrades (SWMG_GetSwoopUpgrade) and per-follower sound tweaks.
- Bezier position keys are interpolated linearly between keys.
- The swoop autopilot (harness only) steers around obstacles and mines with
  a simple look-ahead; it is not a model of the original AI rivals, which
  just ride their tracks.

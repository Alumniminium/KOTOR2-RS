# Session handover (2026-10-09)

For the next agent. Read this, AGENTS.md, docs/architecture.md and
docs/roadmap.md, then the note for whatever area you touch (docs/notes/).

## What this is

kotor2-rs: a Rust/Bevy 0.19.1 rewrite of KotOR2 that reads the user's Aspyr
Linux Steam install in place. GPL-3.0-or-later. No game files in the repo.
The repo lives inside the game folder:
`$HOME/.local/share/Steam/steamapps/common/Knights of the Old Republic II/kotor2-rs`
(install root is `..`). Nothing has ever been committed; commit only when the
user asks. Session goal (Stop hook): "bring kotor2-rs to a playable state".

## User rules (binding)

- ASCII only everywhere: no em-dashes, curly quotes, unicode. Code, docs, chat.
- Never use rm; use trash-put. Root deletions via pkexec may use rm.
- Terse replies, no recaps, no closing summaries, minimal markdown, no bold.
- Do not claim something is fixed unless verified in the user's real setup.
  Say plainly what was only tested headlessly.
- Use pkexec, not sudo. Do not WebFetch her.st.
- The user wants fixes done by agents ("always use agents to fix things").
  Agents share one working tree and one cargo target dir: small focused edits,
  re-read files before editing, never revert others' changes, keep
  `cargo build --release` green.
- The user hates game windows popping up unannounced. Start a game window
  (through k2-mcp or the game binary) only when the user asks. Checks that need no
  window: cargo tests, k2world binaries, logs. See "Windows and workspace 4".
- Use they/them for people whose pronouns are unknown.

## How the user plays and how to restart them

- The game runs from a snapshot binary: `/tmp/k2research/play/kotor2`.
  Never overwrite it except through `tools/restart.sh` (green build only).
- Restart where they left off: `tools/restart.sh --build` (copy at
  `/tmp/k2research/play/restart.sh`). It asks the running game to save slot
  `restart` over the control channel (`~/.local/share/kotor2-rs/ctl/cmd`,
  answer in `ctl/ack`, pid in `ctl/pid`), quits it, snapshots, relaunches with
  `--continue`. Log: `/tmp/k2research/play/session.log` (previous run:
  `session.log.prev`).
- The game also saves slot `restart` on every normal exit, and a `timed`
  slot every 120 s. `--continue` loads the newest of restart, timed,
  autosave, quicksave and save-*. The user lost 90 minutes once because a
  build without the exit save was closed; do not restart them mid-play
  without the save step, and tell them what was loaded.
- Saves: `~/.local/share/kotor2-rs/saves/<slot>/`. Never touch them from
  tests or agents: use `K2_SAVE_DIR=/tmp/k2research/<dir>` (also
  `K2_SETTINGS_DIR`, `K2_CTL_DIR`). Test runs that forget this overwrite the
  user's autosave (it happened with the cage view).
- Game over "Load Last Saved Game" skips saves with a leader down; saving is
  refused while the party is defeated. HK-47 is stored dead (0 HP) in all of the
  user's current saves except save-0001. Unknown whether the user killed
  them; ask if it matters.
- Launch by hand: `WAYLAND_DISPLAY= /tmp/k2research/play/kotor2 --install
  "<install root>" --continue` (XWayland is required: Wayland + NVIDIA
  present-mode bug). The GPU is an RTX 5090 Laptop, wgpu Vulkan backend.
- Hyprland: `~/.config/hypr/hyprland.lua` has rules (title and class match,
  `kotor2` and `kotor2-test`) that float every game window at 800x600,
  centered, on workspace 4 silent. Backups are `hyprland.lua.bak.20261007-*`.

## Windows and workspace 4

Test windows (`--screenshot`, `K2_TEST_WINDOW=1`) use title `kotor2-rs-test`,
class `kotor2-test`; the same rule sends them to workspace 4. If the user is
not looking at workspace 4, Hyprland gives them no frames and runs hang or
return black (vsync-on). Workarounds used: `K2_SETTINGS_DIR` pointing at a dir
with `{"vsync": false}`, one short run at a time with `timeout 90`, fresh
screenshot file names (an existing file makes the run exit early). Animation
cannot be judged from screenshots; the user watches live and reports.

## Build and test

    cargo build --release                 # must print 0 warnings
    cargo test --release --workspace      # all green at last run, except as noted
    k2world all --seconds 60              # 82 modules, 0 panics, 0 VM errors
    k2probe survey                        # format survey

Rust 1.99.0 via mise. Workspace crates: k2-res, k2-mdl (K2-aware MDL reader,
vendored), k2-data, k2-walk, k2-script (NCS VM), k2-world (headless sim),
kotor2 (Bevy game), k2probe. rakata (GPL, K1-only) pinned at rev 6377ce37.

## Done since the playable baseline (this session, newest last)

World and gameplay:
- Open doors keep no collider: DWK vertices are relative to the walkmesh's own
  position (added in k2-walk add_object). Destroyed placeables and doors now
  drop their walkmesh (invisible plc_dblk blockers are destroyed by scripts on
  door open, 105per).
- Doors become passable only after the opening clip length and block again
  as closing starts; line of sight, perception and shooting use the same door
  state (enemies no longer shoot through a door that is still opening).
  Open1 vs Open2 side choice is inferred from the DWK data, not confirmed.
- Plasma torch opens bashable doors and locked placeables on the first swing.
  Attacking a door out of range now pathfinds to a standing point; no reachable
  point or 2 s without progress aborts and returns control. Movement keys
  cancel a bash approach.
- Hostiles now attack: one shared `World::can_see` (range, stealth, line of
  sight) used by OnNotice, perception searches and GetObjectSeen; idle hostiles
  that still see an enemy get a fresh OnNotice every 3 s; perception range
  from the UTC. Creature hide item penalties (Dex -5, to-hit -5) now apply
  (mining droids were far too deadly); blasters no longer count as lightsabers
  for door Hardness. Level 1 fights now cost about 12% HP; may be too easy.
  Class and feat Defense bonuses are not modelled.
- Combat ends 2 s after the last hostile dies (6 s while a hostile sees you).
  A fallen leader passes control to the next standing member; party members
  at 0 HP are knocked out and get up after the battle (combat.md, "Knocked out
  and getting up"); the whole party down shows the original game over screen.
- Saving in combat is allowed.
- Two weapon sets with X to switch, an equip clip (S_Male02 `equip`, draw event
  at 0.567 s) and sounds; off-hand weapons require a compatible one-handed
  main-hand weapon (pairing rule derived from baseitems weaponwield, not
  checked against the original). Equipment slot hover lists the party items
  that fit.
- Quick action bar on doors and containers: Open/Close/Use, Security or Unlock,
  Attack (slots 0, 1, 2). The hover target survives the cursor moving onto the
  HUD.
- Camera: Q/E rotate, right or middle drag orbits, A/D turn, eased yaw and
  focus, swings behind while click-walking. Interact key is F (E now rotates).
- Death: the die clip now plays before the dead loop (cdie/cdead for droids
  and monsters), corpses with loot stay targetable, junk position keys in
  droid death clips are filtered.
- Armor: empty `texL` keeps the model's own textures, head gear (masks,
  goggles) attaches at the head's MaskHook, loading a save re-derives every
  creature's body model and texture (old saves carried a wrong body texture).
- Peragus intro: the Awaken trigger fires after module load, legacy DLG
  animation ids (1000+N, 1400+N, 1200+N) map to `cut{N+1}` clips, PLAYER
  resolves to the party leader, fades and unskippable lines honoured. Kreia's
  stunt model stays lying until the get-up; a displaced stunt is hidden so she
  no longer shows twice; only AnimatedCut or camera-angle-4 dialog lines count
  as scripted staging (other dialogs now turn the speakers).
- cutscenedummy: only a non-zero key counts as an area-space pose (the old rule
  hid 142 objects, including the 101per security desk).
- Force cages: TXI cycle (fix for a junk-prefixed TXI line), water warp, mesh
  UV scroll, per-node alpha clips. User-approved speeds: scroll x60, cycle
  x0.5, water x1 (numpad 1/2, 4/5, 7/8 adjust live). Mip chain for
  single-level procedural textures, frame cross-fade. User said the layers
  still looked low framerate in places; unresolved (possible follow-up: a
  two-phase flow blend for the water layer).
- Swoop racing: Space no longer pauses in a minigame, engine/gear/crash/pickup
  sounds, steering tuned (about 1.2 s per lane crossing), obstacles of type
  mge/mgf are trigger volumes (run OnHitFollower, no damage), race timer
  digits count, shield flash, turret explosion emitters (crates/kotor2/src/
  emitter.rs). Gear shifting is manual again (the user first asked for
  automatic; another change reverted it; confirm with the user which they
  want).
- Audio: default master volume 0.7 (was 1.0).

Tooling and UI:
- Control channel, `--continue`, `--load-slot`, exit save, timed autosave,
  `tools/restart.sh`.
- Graphics options screens (optgraphics, advanced, resolution) wired to
  `GraphicsSettings` in crates/kotor2/src/graphics.rs, persisted to
  `settings.json` (msaa, anisotropy, texture quality, vsync, shadows,
  brightness; grass, force speed effects and frame buffer effects are stored
  but unused). Fullscreen, slider drag and window size under the float rule are
  untested.
- Shadows: one shadow-casting directional light aimed from the nearest room
  light at the leader, shadow sampling in the material, soft blob discs under
  creatures, quality from GraphicsSettings (Off/Low/High). Frame cost not
  measured; Low not inspected.
- Area editor: black screen fixed (the fade overlay was never cleared),
  Load Map (Ctrl+O, all modules), HUD and dialog bars hidden while editing,
  entry framed from above. The UI was then rebuilt on egui 0.36.2 /
  bevy_egui 0.42.0 (editor/ui.rs): object tree, inspector with real widgets,
  menu bar, status bar, palette, gizmo. Tested via scripted input
  (`K2_EDIT_SCRIPT`). Not tested: Revert dialog, View menu kind toggles,
  walkmesh and grid overlays beyond one screenshot, mouse clicks on map list
  rows. The user has only seen the old editor UI so far; they asked for imgui
  and have not yet commented on the egui version (the last editor window was
  just opened for them).
- Test hooks added along the way are documented in the notes (K2_KILL,
  K2_DEATH_SHOT, K2_KILL_LOOT, K2_GUI_HOVER, K2_GUI_USE, K2_SWAP_WEAPONS,
  K2_MG_CRASH, K2_MG_SHOT, K2_EDIT_*, K2_UVDBG, K2_KILL_LEADER,
  K2_GAMEOVER_CLICK, K2_CAM_SPIN, K2_MESHDBG removed).

2026-10-09 (test-verified, not reviewed or staged):
World and gameplay:
- Walk-then-act MCP verbs with door routing: doorway links, and NPC routes
  avoid locked doors.
- Fix: the Bevy Player.pos copy was overwriting world walks.
- Walk animation and follow camera.
- Cutscene hold.
- Door message box.
- locked_at_load door heuristic: unblocked the prison door and Atton's 101atton.

Tooling and UI:
- MCP output size cut. observe returns interactables as a list with directions.
- Tools removed: key_down, key_up, end_dialog, move_to point, wait_until
  combat_end.
- wait_until: default 1 s, cap 30 s.
- key_press holds on game time.
- Speech hold with measured line lengths and a call queue.
- Dialog verbs answer after the next line.
- New test files. crates/k2-world/tests/: cutscene_hold.rs, door_box.rs,
  door_walks.rs, prison_door.rs, route_budget.rs, waypoint_routes.rs,
  doorway_links.rs. crates/kotor2/src/: remote/screen/tests.rs,
  remote/speech/tests.rs, dialog_ui/tests.rs.
- Playtest (k2 MCP, haiku player): reached Atton in 101per through the east
  route (admin console override, then the prison door). Log:
  $HOME/.cache/k2orch/peragus-play/log.md.

## Open items and known gaps

Open from 2026-10-09:
- Review and stage today's work (nothing from 2026-10-09 is reviewed or
  staged yet).
- Rerun talkall (door bare lines are now conversations, so counts change):
  `k2world --install .. talkall`.
- Rerun the doorway probe (baseline 4707 of 6000) and the full CI script,
  $HOME/.cache/k2orch/ci/ci.sh.
- Security desk camera-view pictures are not implemented.
- Rev-walk review change requests not done: use_item and use_power act at
  range (crates/k2-world/src/control.rs, crates/k2-mcp/src/server.rs); skill
  verb defects; leader_clearance against R_MAX 0.45 (crates/k2-walk/src/nav.rs);
  keyboard PLAYER_RADIUS; leftover diagnose_* tests in
  crates/k2-world/tests/control.rs.
- The user must reconnect the k2 MCP server to load the new binary.
- MCP gaps found while documenting (mcp-control.md describes current
  behaviour): take_item, take_all and use_skill are not in the supervisor's
  goal watch (no 300 s cap; a paused game leaves the call waiting); a take
  that walks first leaves the container window open; stop does not clear
  calls held in the speech queue.
- world.md's a_hatchopen / 105PER_T3_End route is written from data reading
  only; no code or test covers it.

User-reported, not yet verified by the user after the fixes (ask or watch):
- Party members knocked out (0 HP) now get up after the battle with 1 HP
  (they were dead for good before). Not verified in play: the timing,
  the get-up clip, droids (see combat.md), the 1 HP amount.
- Death animations: droid and kath hound settled shots, save-loaded and
  scene-placed corpses, and looting a corpse were never checked in the real
  game. The death animation agent was stopped by the user; do not restart that
  agent type unprompted. A reported HK-47 corpse "jitter" was not reproduced.
- Weapon switch animation, shadows and graphics options have not been
  played by the user yet (the running game predates them: restart needed).
- Swoop: freighter and gate-shutter animations never seen on screen; the user
  has not raced since the fixes. Wind sound is requested by the scripts but
  not defined on any track.
- Quick bar: only objects (doors, containers, placeables) get quick actions;
  no creature entries (talk, Use) yet. Icons `i_examine` (Open/Use),
  `isk_security`, `i_attack` were picked by availability, not from the
  original.
- Intro: the floor monitors' power-on animation is not done (lit throughout).
  The kolto tank is a rectangular booth in the data; the user was told and
  offered a round tube (not requested).
- Equip: pairing rule for off-hand weapons is a guess; scripted
  ActionEquipItem gets no animation or sounds.

Larger gaps (from the roadmap and earlier work):
- No full playthrough past the opening modules; 82 modules load headless
  without panics, but script behaviour end to end is unproven.
- Approximated formulas: crafting costs, kill XP, spell DC; class and feat
  Defense bonuses missing; swoop upgrades (SWMG_GetSwoopUpgrade) read 0.
- Trapped doors and containers; stealth, mines and traps polish; influence
  and companion conversations; Pazaak and the other minigames are built but
  lightly tested by hand.
- Editor: main-menu entry button, area names in the map list, pack mods as
  .mod capsules (mods currently go to `~/.local/share/kotor2-rs/mods/<name>/`
  as loose files, played with `--mod-dir`).
- Plasma torch sound, map arrow check, thruster particle fountains, light
  colour and self-illumination clips, emitter support outside minigame models.
- Menu-open world events are not drained, so equipment changes only show on
  the model when the menu closes.

## Workflow lessons

- Reproduce in the user's real flow (their save copy), not an isolated test.
  Several "fixed" bugs came from stale state in saves (body texture, an idle
  droid stuck since a bad notice).
- `--screenshot` needs a fresh file name each run, `WAYLAND_DISPLAY=`, `--mute
  --no-logos --skip-movies`, and a visible workspace 4.
- Agent reports often repeat; read the final one and check the build and
  tests yourself before restarting the user: `cargo test --release
  --workspace` should be clean, `cargo build --release` 0 warnings.
- The user watches things live and answers short; when they send a
  screenshot without text, ask what is wrong in one line.

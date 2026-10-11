# Integration: kotor2 driven by k2-world (crates/kotor2/src/game.rs and UI)

The Bevy binary is a presentation layer over `k2_world::World`. The world
owns objects, scripts, dialog, the walkmesh and combat; Bevy mirrors it.

## Flow

- `WorldRes(World)` is created in `main`. With no `--module` the main menu
  opens (`menu.rs`); New Game enters 001EBO. `--module NAME` starts there
  directly, `--newgame` presses New Game.
- `AppState { Menu, Game }`. In Game, Update runs `GameSet::{Load, Input,
  Tick, Sync, Ui}` in that order:
  - Load: `load_area` after `World::enter_module` (`NeedAreaLoad`). It
    despawns the old `AreaRoot`, spawns LYT rooms under a new one, replaces
    `AreaModule`, moves the leader to a safe spot (101PER's entry is on a
    walkmesh island), and sends `AreaLoaded`. Audio rebuilds music,
    ambience and sound objects on that message.
  - Input: `play::move_player` moves the party leader on `world.walk` and
    calls `set_player_position` (bearing = facing - 90 deg). It only does
    this while `world.player_controlled()` and not in a conversation;
    otherwise the leader follows the world. Moving breaks off a fight.
    `interact`: E or left click on the hover target. Hostile creatures get
    `player_attack`; anything else is used within 1.8 m + radius, or after
    walking there (`PendingUse`). Locked doors and containers get a
    "Locked" bark.
  - Tick: `world.tick(dt)` (real time, capped at 0.1 s), skipped while
    paused, a movie card is up, or a transition is fading. Events go to
    `FrameEvents` for the rest of the frame. While paused (a menu or the
    pause box is open) the tick is skipped but the events are still drained,
    so equipment and armour changes made in menus reach the models at once;
    the weapon draw then swaps models without the clip (the clock is
    stopped). `transitions` takes
    `pending_transition`, fades out for 0.4 s, enters the module, queues
    movie cards, and fades in.
  - Sync: `sync_objects` spawns a root per creature, door and placeable
    (`ObjId`, model child in `ObjBody`) and despawns destroyed ones. NPCs
    follow world position and bearing, smoothed and snapping on jumps over
    3 m. The `Player` marker moves to whoever is party leader, so control
    passes to T3-M4 when 001EBO calls SwitchPlayerCharacter.
    `apply_events` handles door and placeable open/close clips, scripted
    animations (`anim_clips` maps ANIMATION_* to clip candidates), attacks
    (`AttackStarted` anim plus fallbacks) and deaths. `animate_creatures`
    picks clips in priority order: dead, then moving (walk/run from player
    input or `world.motion`, rate matched to walkdist/rundist), then
    scripted, then talking (`tlknorm` for the current speaker), then idle.
  - Ui: dialog panel and camera (`dialog_ui.rs`); fade, movie card, barks,
    damage numbers, HP bars and the HUD line (`hud.rs`); follow camera.

## Dialog

- A bare line (one NPC line with no reply links at all, no camera, no fade,
  dialog not AnimatedCut) is a bark: a bubble from barkbubble_p.gui, no
  camera, no pause (bark.rs; cutscenes.md). A line with a [Continue] reply is
  still a conversation.
- The panel shows the speaker name (tokens substituted), the line, and
  numbered replies (keys 1-9 or click).
- A line whose replies are none or a single empty "[Continue]" advances by
  itself after the voice-over (`audio::Voice::play_voice` duration +
  0.35 s), or after a text-length timer when there is no audio (about 6k
  of 18k VO refs are missing). The DLG `Delay` is used when it is between
  1 and 59. Space/Enter skips. A paused conversation
  (ActionPauseConversation) does not advance.
- Letterbox bars stay up for the whole conversation. Cutscene nodes with
  empty text hide the panel but keep the bars.
- Camera: over the shoulder of the other party, framing the speaker; a
  front shot when the speaker talks to itself (the 001EBO intro is an
  invisible placeable). CameraAngle is ignored.

## Not done

- Keyboard walking is straight-line with sliding (crates/kotor2 play.rs, no
  route through walkmesh.md's nav grid). Object clicks do route: the walk heads
  for a spot within use reach (`World::steer_leader`, with the spots from
  `use_candidates` in k2-world nav.rs, searched by `begin_use_search`).
  Scripted moves and combat approach also follow routes. Creature-creature
  collision (mining droids stack up), real Bink playback, CameraAngle/animated
  cameras, inventory/journal/party GUIs (`ShowGui` events are only logged by the
  world), saving.
- SpeakString volume is not in the event, so the HUD filters out
  `ALL_CAPS_WITH_UNDERSCORES` shout tokens (GEN_COMBAT_ACTIVE) and shows
  the rest.

## Test flags

`--newgame`, `--autotalk TAG`, `--autoreply` (reply 0 every 1.5 s),
`--autotransition` (step into the module's transition trigger),
`--autoattack` (start 5 m from the nearest hostile with a clear line and
attack), `--autouse SECS`, `--autowalk SECS[,YAWDEG]`, `--debug`
(FPS and leader position; F3 toggles), plus `--screenshot PATH --frames N`.
Test hooks fire once per run, with the clock restarting at each area load.

## Conversation facing

- k2-world turns participants when a conversation starts (PC <-> owner) and
  on every entry (node Speaker <-> Listener, resolved by tag; empty means
  owner/PC). It runs before the node's scripts, so a script that sets facing
  itself still wins.
- Only living creatures turn; placeables and doors never do. Nobody turns in
  dialogs with a CameraModel or AnimatedCut: scripts stage those. A
  NotReorienting creature (turret) does not turn to face its partner in a
  conversation, and a bare line (bark, cutscenes.md) turns nobody. Combat
  still turns it.
- SetLockOrientationInDialog(obj, TRUE) is honoured. It is stored in the
  object's local_bools at slot -1001 (`dialog::LOCK_ORIENTATION_LOCAL`), so
  it is saved with the object and no script index can collide.
- The world sets bearings instantly; kotor2 smooths them. While a
  conversation runs, `player_controlled()` is false, so `sync_objects`
  slerps the leader's root (and Player.facing) to the world bearing too.
- Test: crates/k2-world/tests/facing.rs (101per: PC turned away, start a
  conversation, both face each other; a locked PC keeps its bearing).

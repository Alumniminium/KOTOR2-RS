# Cutscenes: movies, conversation cameras, lip sync

## Bink movies (crates/kotor2/src/movie.rs)

- 67 .bik files in steamassets/movies. Bink 1 video plus binkaudio_dct
  (44.1 or 48 kHz stereo); legal.bik has no audio. Sizes vary: logos
  640x480 / 800x600 / 1280x720, story movies 1600x680 at 30 fps.
- Played through an `ffmpeg` subprocess: `-f rawvideo -pix_fmt rgba` to a
  pipe, read one frame at a time on a thread into a bounded channel, shown
  on a fullscreen UI image (letterboxed) paced by ffprobe's r_frame_rate.
  The audio track is decoded to a temp WAV first; video and audio start
  together when it is ready. Steady 30 fps at 1600x680 when the machine is
  not loaded.
- Queue: `game::MovieCards`. Script PlayMovie/QueueMovie (k2-world
  `WorldEvent::Movie`) and StartNewModule transition movies land there.
  Without ffmpeg/ffprobe the old one-second name card is used.
- While a movie is up the game holds: the world does not tick, dialog line
  timers and input stop (`movie::movie_up`), `Time<Virtual>` is paused
  (animation, sound emitters, cutscene cameras), plain sinks (music,
  ambience, voice, UI sfx) are paused and spatial sinks are muted, every
  frame so sinks started mid-movie are caught too. All of it is restored
  when the movie ends or is skipped. Logged as "holding game audio" /
  "during ..." / "released game audio".
- Why this matters: 001EBO's intro dialog calls PlayMovie (permov01, the
  opening crawl) from entry 3. Before the hold, the conversation kept
  advancing under the movie and its VO played over it.
- Esc, Space, Enter or a left click skips; those inputs are swallowed while
  a movie is up. Launching to the main menu plays legal, leclogo,
  obsidianent, aspyr first (order not verified against the Aspyr build)
  unless `--no-logos`. Test flags: `--movie NAME`, `--skip-movies`.
- Scripts referencing movies (by string scan): 001ebo permov01/02, 003ebo
  permov01-07 plus others, 107per permov04/05, 201tel TelMov03-06, ...

## What holds the party (k2-world lib.rs party_held)

`World::party_held()` is true while a cutscene holds the leader:

- a conversation (animated and paused ones too);
- input blocked by a script: `input_blocked_for`, set by NoClicksFor, and
  kept at 0.25 s each frame by a minigame (minigame/mod.rs);
- a queued action on the leader other than Attack (its own attacks do not
  count);
- a leader that cannot act: not commandable, dead, downed, or no creature.

There is no SetCutsceneMode in this nwscript (checked against the shipped
nwscript.nss); the holds are the ones above.

Fades do not hold the party. SetGlobalFadeOut, SetGlobalFadeIn and
SetFadeUntilScript only emit WorldEvent::FadeOut or FadeIn (host.rs).

Movies are not in party_held. The keyboard step and the click walk (play.rs
move_player) stop while any card is in MovieCards, name cards included (no
ffmpeg). Control walks use `movie::movie_up` (Env.movie, real movies only), so
a name card does not stop them.

Control walks (control.rs `held`) use `walk_held()`, which is party_held with
a door's box not counted as a conversation. A walk started while held answers
"refused: a cutscene is running"; a cutscene that starts during a walk answers
"interrupted: a cutscene started". A box raised mid-walk does not interrupt
it. wait_until and an Exit that already left the module are not cut off. The
keyboard and click walk do stop for a box (the conversation holds them).

## Conversation cameras (crates/kotor2/src/cutscene_cam.rs)

CameraAngle on entry nodes, over all shipped DLGs:
0 (39113, auto), 1 (855, face), 2 (42, shoulder), 3 (109, wide),
4 (85, animated), 5 (62, no change), 6 (3023, static).

- 6 static: GIT CameraList entry by CameraID. Transform is
  `T(Position + Height * Z) * R(Orientation) * Rx(Pitch deg)`, camera
  looking down its local -Z with +Y up, so Pitch 90 is level (shipped
  values are 63-100). Orientation is a pure Z rotation, scalar first.
  FieldOfView is vertical degrees (50-80). Checked visually: 001EBO camera
  7 frames T3-M4 in the cockpit, camera 30 looks down at the medbay bed.
- 4 animated: the DLG CameraModel (18 models such as 001ebocam, 101percam,
  301narcam) has a root at the area origin and one animated node,
  `camerahook`, same -Z forward / +Y up convention. Clips are CUT001W,
  CUT002W, ...; CameraAnimation 1200+n plays CUT{n+1:03}W from the start
  (101kreia uses 1201 = CUT002W, the only clip in 101percam). Values 1000,
  1001, 1401 and 10098 appear too; they are treated as "keep the running
  clip" (not verified). Position keys are offsets from the hook's rest
  position, as in creature animations.
- 1 face: close-up of the speaker from the listener's side. 3 wide: both
  participants from the side. 5: keep the previous shot. 0/2: the
  over-the-shoulder camera in dialog_ui.rs.
- Test flags: `--autodialog DLG[,OWNER_TAG]`, `--force-angle N`.

## Participant animations

Node AnimList: participant tag (empty = owner) plus Animation id. Ids
10000+n are rows of dialoganimations.2da (name, looping, fireforget, ...);
the name matches animations.2da `description`, whose `name` is the clip
(Talk_Normal -> tlknorm, Nod_Yes -> nodyes, Scanning -> scanning). A few
need a manual map (Listen -> listen, Sit_Chair* -> sit, ...). Ids below
10000 (1011-1604, e.g. 1412 x280, 1428 x98) are not understood yet and are
skipped. Looping clips hold until the next line.

## Peragus intro (101per, dialog 101awake)

How the original runs it (disassembly of the NCS, DLG and GIT data):

- Trigger `Awaken` (newgeneric020) sits on the module entry point; its
  OnEnter k_awaken fades to black (SetGlobalFadeOut + SetFadeUntilScript),
  and 1 s later `inv_talker` (an invisible placeable) starts 101Awake with
  the PC. A local boolean on the KolTank placeable guards against a repeat.
- Entry 0 runs a_start (jump the PC to WP_player_start in the tank, fade in
  after 1 s over 2 s, stop the music) and a_room_anim; entry 10 runs
  a_move_player (JumpToLocation to WP_wakeup_donnie on the floor, local
  boolean 29 on the PC). Cameras are GIT CameraList ids 33, 6, 24, 39, 32,
  34, 35 (all static, CameraAngle 6). Nodes with FadeType 3 fade in from
  black, FadeType 4 fade out (durations are not in the file; we use 0.8 s,
  the fade-out ending with the line).
- Participant "PLAYER" is the PC (it has no usable tag). Its Animation ids
  are legacy cutscene ids: 1000+N is the one-shot clip cut{N+1:03} and
  1400+N the looping clip cut{N+1:03}L of the creature's supermodel chain
  (S_Female02 for the male PC too). 1409 floating in the tank = cut010L,
  1410 twitching = cut011L, 1011 sinking to the knees = cut012 (2 s, last
  frame held), 1412 face down on the floor = cut013L, 1013 getting up =
  cut014 (8 s, the node waits it out via Delay 8). Checked visually and
  against other DLGs (Sion 1027/1428/1029 = cut028/029L/030, the Sith
  assassins 1014/1415 = cut015/016L).
- The end reply runs a_intro_autosave (an empty stub in the data), then
  control returns with the PC standing on the floor spot.
- Not done: the room animations (a_room_anim -> PlayRoomAnimation
  101per2b: scriptloop01/02/03 fade the floor monitors medmon01-28 from off
  to on over 9.7 s; rooms have no AnimPlayer, so the monitors are lit
  throughout).

What was wrong in ours: (1) enter_module marked every trigger the party
starts inside as already entered, so Awaken never fired; (2) the client
nudges the leader off the tank's collision before the first tick, which
would have stepped it out of the trigger anyway, so triggers now run once at
the end of enter_module from the entry position (transition triggers stay
primed; revisits prime all); (3) the legacy animation ids were skipped and
"PLAYER" never resolved, so the PC stood in the tank and on the floor;
(4) FadeType, NodeUnskippable and Delay-longer-than-voice were ignored.

Test hooks: `K2_DLG_SHOT=PATH,ENTRY:SECS,...` (with K2_TEST_WINDOW=1)
screenshots SECS into entry ENTRY, `end:SECS` after the conversation.
Headless test: crates/k2-world/tests/intro.rs (entries 0..18 in order, PC
jumped to the floor at entry 10, no control during, control after).

## Peragus: Kreia wakes (101kreia entries 41-43)

- Closing the loot window of the corpse container (g_tresgencorp004) runs
  its OnClosed a_setkrespwn: fade out, destroy the `kreia_corpse` placeable,
  CreateObject p_kreia002 (tag KreiaStuntman, appearance 540 = model
  p_kreiastunt) at WP_KREIA_SPAWN1 and start 101kreia with it as owner.
- p_kreiastunt is a stunt model: its own clips key `cutscenedummy` at the
  corpse's area coordinates (-21.99, 12.30, 9.11). CUT001L is the lying pose
  (0.03 s loop), CUT002W (9.03 s) is Kreia getting up, filmed by the
  101percam clip CUT002W. Entry 41 (Delay 9, camera angle 4) lists OWNER
  animation 1201; legacy ids 1200+n are the stunt's cut{n+1}w clip, like the
  camera animation ids.
- Entry 43's reply script a_cut_kreia(1) creates the standing p_kreia at
  WP_KREIA_SPAWN1 and starts 101kreia again with it as owner (the regular
  talk, which also destroys the stunt, a_cut_kreia(5)).
- Ours: creatures whose body model is a stunt use their own clip as the idle
  and are posed in area space (as the placeable corpse already was); a
  stunt never falls back to pause1 (which stood it at the area origin), and
  a legacy one-shot (the get-up) is held through the following lines. The
  whole dialog used to count as scripted staging because it has a
  CameraModel, so nobody ever turned to face anybody; only AnimatedCut
  dialogs and animated-camera lines (CameraAngle 4) do now.
- Hand-over: at the end of entry 43 (screen black) a_cut_kreia(1) creates
  the standing Kreia and jumps the stunt to MN_MEDLAB; a_cut_kreia(5), at
  the next conversation's first line, destroys the stunt with a 1 s delay.
  A stunt's pose is in area space and ignores its root, so the jump did not
  move it on screen and both Kreias stood there for that second while the
  fade-in showed them. A stunt that moved more than 1 m off its creation
  point is hidden now (`StuntOrigin` in game.rs). Overlap of Kreia models
  on the stage: 31 ticks (1.03 s) before, 1 tick (0.03 s, under black)
  after; the corpse placeable and the stunt are destroyed and created in the
  same tick, so that swap is atomic.
- Headless test: crates/k2-world/tests/kreia_wakes.rs (entries 41, 42, 43,
  then the standing Kreia's conversation with PC and Kreia facing each
  other). The pose and camera were not looked at on screen.

## Lip sync (crates/kotor2/src/anim/lipsync.rs)

- LIP V1.0: "LIP V1.0", f32 length, u32 count, count x (f32 time, u8
  shape). 14851 files in lips/<module>_loc.mod (77 capsules), named like
  the VO resref (e.g. 101101atton001).
- Heads have no per-viseme models: the humanoid supermodel S_Male02 has a
  0.5 s `talk` clip rooted at `talkdummy` with 16 keys per face node
  (f_jaw_g, f_um_g, f_lmc_g, f_rmc_g, f_Rlm_g, f_Llm_g, f_tonguetip_g).
  Key s is viseme s. We sample `talk` at s * length / 15 for the two LIP
  keys around the current time and blend, after the body animation.
  An `emotions` clip of the same shape exists (FacialAnim?), unused yet.
- Lines without a LIP file get a synthetic track from the text (letters to
  visemes, ~11 per second). Alien-set lines (audio.md) have no LIP in 501ond;
  their synthetic track runs for the clip that is playing, to within one
  0.09 s step.
- Viewer check: `kotor2 --viewer pmbam --head pmha01 --lip 101101atton001
  --dist 0.55 --focus 1.66` shows FV, OOH and OH shapes at the logged
  times.

## Audio notes

- Master volume is applied twice for audio.rs Voice and sfx sinks
  (GlobalVolume = master, plus a per-sink Volume of master), so `--volume
  0.05` plays at 0.0025. Harmless at the default 1.0. Movie audio relies
  on GlobalVolume only.

## Holograms (console logs)

How the game does it (101per, checked by disassembly):

- A log console is a placeable whose DLG (medlog, secter) ends a log choice
  on a reply script (a_medoffdlg_fire). It CreateObject's a creature UTC
  (n_commf001, tag MedOff) at a waypoint (WP_MEDOFF, (21.0, 52.5)) about
  60 m away in an out-of-bounds "holo room", then after DelayCommand(0.1)
  AssignCommand(MedOff, ActionStartConversation(PC)) with the creature's
  own DLG (medoff), whose start nodes are gated by a global set by the
  console reply (101PER_Med_Log). The hologram's last reply jumps back to
  the console dialog (a_medlogdlg_fire); logging out destroys it
  (a_delmedoff).
- The creature UTC has the K2 field `Hologram` = 1. That flag, not a script
  effect, is what makes it render as a hologram. (VFX_DUR_HOLO_PROJECT 9010
  also exists in nwscript but these logs don't use it.)
- Head models flag their inner meshes (teethLWR, teethUPR, tongie,
  teethlower) with the trimesh `hide_in_holograms` byte (part of the 8 K2
  header bytes); those are not drawn on holograms.
- The default camera (angle 0) frames the speaker alone: the speaker is far
  from the PC, so an over-the-shoulder shot would look through walls.

Ours:

- k2-world `CreatureState.hologram` from the UTC; kotor2 `holo.rs` puts
  `HologramLook` on the object root and swaps every mesh material below it
  for a hologram variant (shader FLAG_HOLO: luminance in projector blue,
  scanlines, shared flicker), hiding `HideInHolo` meshes.
- The hologram pass is alpha-tested opaque, not blended: it writes depth,
  so nothing behind the front surface shows through; every third scanline
  is discarded for the see-through look. An additive version showed the
  mouth interior and far-side geometry through the face.
- dialog_ui's default shot and cutscene_cam's face/wide shots fall back to a
  solo shot of the speaker when speaker and listener are more than
  `SOLO_SHOT_DIST` (8 m) apart.
- `k2world talk` follows up to 5 follow-up conversations that scripts start
  after the first one ends (console -> hologram -> console).
- Viewer: `--holo 1` draws the model as a hologram.

Not done: VFX 9010 applied by script (WorldEvent::VisualEffect is not
consumed on the Bevy side yet), hologram removal by effect expiry.

## Barks: one-liner conversations (dialog.rs, bark.rs, barkbubble_p.gui)

A conversation whose starting line is bare is played as a bark, not as a
dialog. Bare means `Dialog::is_one_liner` (k2-data dlg.rs): the entry has no
reply links at all. Any reply link, even a single "[Continue]" that leads
nowhere, makes a real conversation (the galaxy map glxy_map and the prologue
controls dialog are conversations this way). Cutscene lines never qualify:
a camera model, AnimatedCut, a node FadeType, or a node with CameraAngle 4
(an animated camera; the survey finds no bare CameraAngle-4 entry in the
install, so this only makes the rule consistent with `cutscene_line`).
Conditions on the start link
are evaluated as usual, so a dialog with several starts is a bark when the
start that passes is bare.

What happens (`World::start_conversation`, `speak_line`):
- The line's own scripts run, then the dialog's end script and the owner's
  end-of-dialogue event (ScriptEndDialogu / OnEndDialogue). No conversation
  state is kept (`World::conversation` stays None, no ConversationStarted or
  ConversationEnded), nobody turns, there is no dialog camera and nothing
  pauses. GetIsConversationActive is FALSE while a bark plays.
- The line goes out as `WorldEvent::BarkLine` (speaker, dialog, text with
  tokens substituted and braces stripped, VO, sound, alien race, emotion,
  Delay). Empty text emits nothing but still runs the scripts.
- `talk_to` falls back to the creature's conversation only when its
  ScriptDialogue did nothing for this NPC: no bark by it (`World::barks_by`)
  and no conversation it started. A StartConversation the script queued for
  this NPC is the talk when the queue was fresh (empty, or cleared by
  ClearAllActions) and the start is at its front; it runs when it reaches
  the front (tick.rs run_actions). k_ai_master queues one for computer-type
  NPCs, after ClearAllActions (NCS 1b73 and 1bd9), so the talk begins on the
  next tick. talkall ticks up to 3 times for such owners only, before it
  counts them as queued.

Presentation (crates/kotor2/src/bark.rs): the bubble is barkbubble_p.gui,
opened with Placement::Center. The root panel (the teal border) is drawn only
for a centred layout; Placement::Anchor skips it, as the HUD root needs. The
line goes in LBL_BARKTEXT. Duration: the VO (else the sound, else the alien
set) plus 0.35 s, a Delay of 1..59 if longer; with no audio the dialog's
reading time (`dialog_ui::read_time`). Only the newest bark of a frame plays
(each play_voice stops the one before it, but only sees voices spawned in
earlier frames). Barks that land during a conversation are dropped. A new
bark replaces the bubble. The countdown uses real time and stops while the
pause menu or a movie is up; a conversation takes the bubble down.

Position: ActionStartConversation's nBarkX / nBarkY (args 12, 13; -1 = the
layout's own place) move the whole layout so its root's left, top corner sits
there (`GuiScreen::set_origin`, layout px, the 800 x 600 space). Carried as
`Action::StartConversation.bark` and `WorldEvent::BarkLine.pos`. The units are
an assumption (layout px, not window px).

Survey (`cargo test --release -p k2-world --test barks survey -- --ignored
--nocapture`), 2026-10-09: over all modules, 53 creature and placeable
dialogs have only bare starts, 1218 have a real conversation start (counted
per object, not per DLG). The bare ones are the caged animals (501ond tame)
and Telos bystanders (200comf, 200comm, 200ithr, 200merc, 200tsff, 200tsfm,
202czerf, 202czerm, 203_ith_pc). Iziz citizens, soldiers and workers have a
[Continue] reply, so they are conversations, as are the turrets (turret.dlg).

Subtitles (survey `survey_lines_without_subtitle_text`): the dialog panel
shows the line whenever its text is not blank, so a line with VO and no text
is the only silent case. Across all modules 135 entries have audio but no
text and 1051 have neither (camera and note-only cutscene lines). In 501ond
the audio-only entries are cutscene lines (cs_tal_1, mandalore) and none is an
ambient NPC; the citizens' lines have their text (k2world talk prints it).
So the ambient "no subtitles" report is not explained by empty text; not
reproduced in the client (no game window was used).

Not verified against the original: which bare lines the original shows as a
bark bubble and which as a dialog window. The rule above is the working
assumption. nwscript.nss describes barks as conversations ("nBarkX and nBarkY
override the left, top corner position for the bark string if the
conversation starting is a bark string", on ActionStartConversation).

SpeakOneLinerConversation(sDialogResRef, oTokenTarget) (nwscript 417,
"immediately speak a conversation one-liner") says the dialog's first passing
line now, as a bark, spoken by the caller; the default dialog is the caller's
own. Only the line's own scripts run; its reply and the end scripts do not.
oTokenTarget is ignored: the line's tokens resolve as for any line (the
global CUSTOM tokens and the PC's name), not against that object. No shipped
script calls it, so it is compiled but not exercised.
SpeakString, ActionSpeakString, ActionSpeakStringByStrRef, BarkString and
ActionBarkString stay floating text over the speaker (hud.rs,
`WorldEvent::Bark` / `Speak`); a BarkString on OBJECT_INVALID ("general bark")
shows nothing, since there is no head to float over.

Tests: crates/k2-world/tests/barks.rs. A caged animal's bare line gives one
bark and no conversation; a citizen's [Continue] line is a conversation; the
galaxy map (glxy_map) and the prologue controls dialog start conversations;
the turret keeps its bearing and talks its line as a conversation; 501ond's
bare and real dialogs classify as above; a bark carries nBarkX / nBarkY.
crates/k2-data/tests/dlg.rs checks the cutscene exclusions (camera model,
AnimatedCut, fade, CameraAngle 4) and the reply-link rule with unit tests.


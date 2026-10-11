# Remote control (MCP)

An AI agent plays KotOR 2 in a real window, through an MCP server. The
server is a thin supervisor; the game does the playing.

Three parts:

- `k2-mcp` (crates/k2-mcp): a stdio MCP server. It builds the game, launches
  the snapshot in a window, keeps the game's output, and forwards commands to
  the game over its control socket. It holds no game logic, so it can stay up
  while the game is rebuilt and relaunched. `policy.rs` checks what a caller
  may pass to the game.
- The game (crates/kotor2): with `--control` it serves the control socket each
  frame. `remote.rs` answers host verbs (input, pick, project, screenshot, log
  filter, trace, info, save and load, quit) and runs the call queue.
  `remote/screen.rs` says what on screen takes input, and `remote/speech.rs`
  holds calls while someone speaks. `remote_input.rs` plans keyboard and mouse
  input. `logging.rs` is the logging subscriber: its console filter and its
  trace file can change while the game runs.
- The control API (crates/k2-world, `control.rs` and `control/socket.rs`): one
  serde command set. Semantic verbs (observe, move_to, interact, choose_reply,
  ...) run on the World through the same paths a player takes. Host verbs
  (keys, mouse, pick, project, screenshot, logs, trace, info, quit) are answered
  by the game process.

Both ways of playing act on the same game: semantic verbs, or the input verbs
(screenshot to see, pick or project to locate, then click and type).

## Launching

`game_start` runs `cargo build --release -p kotor2` in the checkout (`build`
false skips the build and relaunches the last snapshot, or is refused when there
is none yet). A red build is reported (the last 120 lines of cargo's output) and
nothing launches.
Cargo runs in its own process group, so a shutdown of the server stops a build
in progress. A green build is copied to `<root>/bin/<UTC time>/kotor2`, with a
`build.json` (time, `git describe`, warnings, size). The snapshot folder is
owner-only. The game runs from that copy, so later cargo builds cannot replace
the file a running game uses. The newest 5 snapshots are kept; older ones are
moved to the trash with `trash-put`, never deleted outright, and the one a
running game uses is never moved. If trash-put is not installed nothing is
moved, and the server logs that on stderr.

`start` chooses how the game starts:

- `menu`: the main menu (no start flag).
- `new` (the default): a new character through the prologue (`--newgame`).
- `chargen`: character creation from the main menu (`--chargen`). It is not
  the Quick defaults.
- `continue`: the newest save (`--continue`).
- `load:<slot>`: a save slot (`--load-slot`).
- `module:<name>`: a module, skipping the menu (`--module`).

Extra game flags (`args`) pass an allowlist. Switches: `--control`,
`--newgame`, `--continue`, `--chargen`, `--quickstart`, `--debug`,
`--autoattack`, `--mute`, `--skip-movies`, `--no-logos` (the last three only
change sound and the intro). Flags with one value: `--load-slot`, `--module`,
`--autowalk`, `--autouse`, `--autotalk`, `--autoattack-door`, `--cam`,
`--player-appearance`, `--volume`. `--autoreply`, `--autotransition` and `--fly`
are refused too. Anything else is refused before a build starts. That
includes the headless entry points (`--sfx-survey`, `--audio-survey`,
`--audio-test`, `--edit`, `--edit-selftest`, `--mod-dir`, `--mod-name`,
`--screenshot`) and `--install`, which the supervisor sets itself. A NUL byte
in an argument is refused.

The game is always windowed. The launch environment:

- `WAYLAND_DISPLAY=` (empty): XWayland, which the NVIDIA present mode needs.
- `K2_TEST_WINDOW=1`: window title `kotor2-rs-test` and class `kotor2-test`,
  which the user's Hyprland rule floats onto workspace 4 silent. The window is
  on a hidden workspace, so it may not get frames: keep workspace 4 visible
  when you need screenshots.
- `K2_SAVE_DIR`, `K2_SETTINGS_DIR` (with `settings.json` set to
  `{"vsync": false}` on first launch), `K2_CTL_DIR`, `K2_SHOT_DIR`,
  `K2_TRACE_DIR`, `K2_INSTALL`: all under the run root. The player's saves and
  settings are never used.
- `K2_MOD_DIR=<root>/mods`: the game's loose mod folder is the run's own, which
  starts empty. No player mod folder is read.
- `NO_COLOR=1`.
- `--install <game folder> --control`, then the start flag, then the extra
  flags.

The caller's `env` (game_start) is applied first, and the isolation variables
above are set after it, so they win. An `env` name is refused with an error if
it is empty or contains `=` or NUL, if it starts with `K2_`, or if it is
`WAYLAND_DISPLAY` or `HOME`, or starts with `XDG_` or `LD_`. Names are compared
without regard to case, and a value may not contain NUL. HOME and XDG_* are
refused because the game falls back to them for its data folders; every folder
the game uses is set explicitly, so it does not need them. No K2_ variable can be
passed: the game's K2_ knobs are debug and test switches. Other names pass
through unchanged, including DISPLAY, XAUTHORITY (the X session the XWayland
window needs) and PATH (the tools the game's build and run use).

The run root is `/tmp/k2research/mcp`, or `K2_MCP_ROOT` when set. The root is
refused when it is inside the player's own data folder (`$XDG_DATA_HOME/kotor2-rs`,
or `~/.local/share/kotor2-rs`), when the root itself is a symbolic link, or when
it contains `:` (the game splits K2_MOD_DIR on `:`). A link above the root is
fine, for example a root under a symlinked home: the player-folder check resolves
links, so a link into the player's folder is refused too. It is repeated after
the folders are created.

An existing root is used as it is, after checks: it must be a folder you own,
not writable by others, and either empty or a run root. A run root holds the
layout marker file `layout` (containing `1`) and nothing but `server.lock`, the
marker and the layout folders, each of which must be a folder. A root with
anything else is refused before anything is created, so an existing folder such
as `~/Documents`, or a folder with a file named `logs`, is never filled with run
folders. A root made by an earlier server, without the marker, is refused too:
add a `layout` file containing `1` by hand after checking it.

Every folder above the root, up to `/`, is checked: the root's parent must be
owned by you, and each folder above it by you or by root. None may be writable by
others unless it has the sticky bit (so `/tmp` passes). Missing folders above the
root are created owner-only, and only after these checks. This keeps an
attacker's folder anywhere on that path from swapping the root out.

Permissions: the folders the server creates (the root, the layout folders, and
each snapshot folder under `bin/`) are mode 0700. Folders that already exist are
never chmodded: an existing root keeps its mode. A layout folder that already
exists must not be writable by others (group or other write bits refuse the
launch); its mode is otherwise left alone. The lock file, the layout marker, each
run's log file and the settings file are mode 0600. A link anywhere in the run
tree (for example a save or a setting that points outside it), or an entry
another user owns, refuses the launch. The tree is checked before anything is
created, so a refused tree leaves no folders behind. The layout marker is written
right after the root is created, so an interrupted create leaves a root that the
next start accepts.

One server per run root. The server holds an exclusive lock on `<root>/server.lock`
for its life. A second server on the same root exits at once with an error. The
game's control socket is cleared before a launch only when it is stale: a socket
that still answers belongs to a running game, and the launch is refused.

Layout under the run root: `bin/` (snapshots), `saves/`, `settings/`, `ctl/`
(control.sock), `shots/` (PNG), `traces/` (JSON), `logs/<run>.log` (all output
of one run), `mods/` (K2_MOD_DIR, empty).

Restart (`game_restart`): builds first (a red build leaves the game running, and
the game keeps playing while it builds). With no game running, it saves nothing
and starts the newest save (`continue`). Otherwise it saves to `mcp-restart`,
right before the stop, so the progress made during the build is kept. A save that the
game refuses aborts the restart, and the game keeps running. Then it stops and
relaunches with `load:mcp-restart` and the extra flags of the last start. The
order save, stop, relaunch is tested over stdio. The build step is not observable
there, since K2_MCP_GAME_BIN skips the build.

Stop (`game_stop`): sends Quit, which runs the game's normal exit save. If the
game does not exit within 20 s, SIGTERM (the game saves on that too), then a
kill after 10 s more. `game_stop` waits for a start or restart in progress.
`game_start` and `game_restart` are refused while another start, restart or
stop is running.

Start reply: the window size is reported only once the game says the window
has settled (it waits up to 20 s). Until then `game.window` is left out and
`game_status` reports the size once it is settled. The reply's `settled` says
which case it is.

## Signals and orphans

SIGTERM, SIGINT and SIGHUP sent to k2-mcp end the session the same way as the
client closing stdin: a build in progress is stopped, the game it started is
stopped (quit, then SIGTERM, then kill), and then k2-mcp exits. A launch still
waiting on its build starts no game.

The game is spawned by a launcher thread that lives as long as the server, and
the child is set to receive SIGTERM when its parent dies (PR_SET_PDEATHSIG).
SIGKILL cannot be caught, so a killed server leaves the game to exit on the
signal instead: the game saves on SIGTERM like any other exit. A cargo build
that a killed server leaves behind finishes on its own and starts nothing.

## Reply text

The server sends text only, with no envelope. A call that succeeded gives its
data as one line of JSON, or its message when it has no data, then the game's
messages, each once. A failed call gives its message alone and is marked as a
tool error. Floats are rounded to two decimals everywhere.

observe is compact text. It starts with the open line when a window or screen is
up (`open: ...`, also in its data as `open`). Then a header (module, area, `t=`
game time, `leader=`, `paused`, `game-over`), one line per party member
(`id tag "name" lvl=N hp=cur/max`, then `fp=` and `xp=` when set, `leader`,
`dead`, `combat`, `cond=[...]` and `gear=[slot:name, ...]`), then the
interactable objects nearby, nearest first. Each is
`id name (kind, flags) - distance and direction`, for example
`407 Medical Computer (placeable, talks) - 3.3 m ahead and to your right`. The
direction is relative to the leader's facing, in one of eight sectors, and
", above" or ", below" is added when the height differs by more than 1.5 m.
Flags: `open` or `closed` (doors), `locked`, `talks`, `hostile`, `dead`,
`trapped`, and `scripted` (the object has an OnUsed script; the data's
`scripted_use` says so).

Interactables are usable doors (open or closed), usable placeables and placeables
that talk, creatures that are alive or talk, items on the ground, and trapped
triggers. Scenery and helpers (chairs, desks, inv_talker, k_trans_abort) are left
out. At most 150 objects in range are read, nearest first, and the interactables
among them are listed.

Then the conversation (`conversation <dialog> speaker=<id> "<name>": <text>`,
with a `  N: text` line per numbered reply), combat (`combat engaged=[...]`),
journal (`quest "name" state=...`), inventory (`item <id> "name"`), level-ups and
credits (when not zero), each only when set. The default radius is 12 m.
verbose=true adds raw positions (`pos=` on party lines, `at=` on nearby ones),
the Force powers and the journal text.

game_status leaves out the log filter, the socket path, the physical window
sizes and the full build records unless verbose=true.

key_press is the exception: its success text is the game's account of the hold
(the held time, the metres moved, and any reason for an early release), then
`position=[x,y,z]`, then the game's messages. Its wait is its hold in game time at
up to ten times slower than real time, plus a minute, plus any speech hold the game
reports (see Speech). Goal calls (move_to, interact, talk_to, transition, wait_until)
wait up to 300 s plus any speech hold. The other calls have no limit, and
choose_reply and continue_dialog are answered once the next line is up (see
Speech).

## Tools

Supervisor: `game_start`, `game_stop`, `game_restart`, `game_status`,
`logs_tail`.

Logs and trace: `set_log_filter` (the console filter), `trace_start` (its own
`path` and `filter`), `trace_stop`.

Screen: `screenshot` (returns the PNG as an image block, with its path, width,
height and scale_factor).

Input: `key_press` (hold_ms sets the hold), `type_text`, `mouse_move`,
`mouse_click`, `mouse_drag`, `scroll`.

Pointer queries: `pick` (the object under a window position, as {object, name,
kind}) and `project` (an object's window position, as {x, y, on_screen}).

Semantic verbs: `observe`, `inspect`, `journal`, `list_saves`, `stop`,
`move_to`, `interact`, `talk_to`, `transition`, `choose_reply`,
`continue_dialog`, `attack`, `use_power`, `use_item`,
`use_skill`, `equip`, `unequip`, `take_item`, `take_all`, `switch_leader`,
`set_party`, `level_up`, `wait_until`, `save`, `load`.

Debug, not human-equivalent (their descriptions say so): `debug_teleport`,
`debug_set_global`, `debug_give_item`. They are offered only when the server
process itself runs with `K2_MCP_DEBUG=1`. Without it they are not listed.

Resources: `k2://state` (the observation as JSON, radius 12 m) and `k2://journal`
(the journal as JSON).

Arguments and limits (defaults in brackets): game_start `start` [new], `build`
[true], `args`, `env`; game_restart `build` [true]; game_status `verbose`
[false]; logs_tail `n` [100, at most 2000], `filter`, `since`; observe `radius`
[12 m], `verbose` [false]; move_to `object`, `run` [walk], `timeout` [60 game
s]; wait_until `condition` (idle, dialog or area_loaded), `timeout` [1 game s, at
most 30]; key_press `key`, `modifiers`, `hold_ms` [60, at most 10000]; type_text
`text` (at most 200 characters), `delay_ms` [30, at most 1000]; save `slot`
(`mcp-` and letters, digits, `-` or `_`) and `name`; set_party at most two
followers; screenshot and trace_start `path` (a bare file name under shots/ or
traces/, or a path inside it; an existing file is refused); trace_start `filter`
[info,kotor2=trace,k2_world=trace].

`game_status` reports whether the game runs (`running`, and `mode` windowed), its
run (pid, run_id, uptime_s, start, build stamp, log file), what the game says about
itself (the info reply: `phase` (menu, movie, loading, dialog, minigame, paused or
game), `state`, `module`, `time`, `paused`, `speaking`, `speaking_left_s`,
`speaking_total_s`, `hold_left_s`, `window`, `log_filter`, `trace_file`), the last
build (left out while a game runs) and the last exit: `stopped`,
`exited_on_its_own` or `crashed`.

## Busy gate

One goal runs at a time: move_to, interact, talk_to, take_item, take_all,
use_skill, transition, wait_until.
One input job runs at a time: a key, type or mouse action, and pick. A screenshot
is a job too, until its PNG is written. Calls that do not fit are refused at once
with a "busy" message, not queued.

While a goal runs, these still answer: observe, inspect, journal, list_saves,
game_status (info), project, screenshot, set_log_filter, trace_start,
trace_stop, stop and quit. Keys, typing, pointer input, pick, save, load and the
other goal verbs are refused ("busy: move_to is still running; observe, or
stop").

While an input job runs, the read-only calls (observe, inspect, journal,
list_saves, game_status, project), stop, quit, set_log_filter, trace_start and
trace_stop answer. Screenshot and the other verbs are refused ("busy: an input
job is still running; observe, or stop"). While a screenshot is written, the same
calls answer and the others are refused ("busy: a screenshot is still being
written; observe, or stop").

`stop` cancels the running goal or input job, cancels a queued module change,
and clears the leader's queued actions.

## Speech

While an NPC line is being spoken, or a bark or one-liner bubble is up, no command
takes effect. Only stop, quit and game_status run during speech: stop runs at once
and cancels a running goal or input job, but the speech goes on. Every other call
is held, not refused, and runs once the speech ends, reads included. Its reply
starts with the wait, for example "waited 3.2 s for Kreia to finish speaking; ...",
and carries `waited_s`. A call still held past its cap is refused ("refused: Kreia
is still speaking after ...; the call was not run").

A line's measured length is taken when it starts: its decoded voice-over plus
0.35 s (or its node Delay when that is longer), or, with no voice-over, its text
time: the node Delay when that is 1 to 59 s, else 1 s for an empty line, else 1.2 s
plus 0.055 s per byte of text, kept between 2 s and 12 s. A bark is measured the
same way. A held call waits at most the seconds left in that line plus 2 s, so a
call made at the start of a line waits the line's measured length plus 2 s. A paused
game does not play the line on, so a call held while paused is refused at its cap.

When a call starts waiting, the server sends an interim frame at once: a reply with
data `interim` "held", `wait_s` (the seconds it may still wait) and `speaker`, and
the message "held while Kreia speaks; the call runs when it ends (up to 3.2 s)". The
final reply follows; socket clients skip the interim frame and extend their read
limit by `wait_s`. Calls queued behind a waiting one get the same frame once.

A message box (a door's OK box) is not speech and holds nothing. While one is up,
reads answer at once, and actions are refused ("refused: a message box is open;
close it with continue_dialog, or key_press Escape"). The box closes with OK, Enter,
Escape or continue_dialog.

choose_reply and continue_dialog answer once the next line has been spoken and its
replies are up (or a message box is up), or the conversation has ended. The answer
carries the conversation as it stands then (speaker, text, numbered replies, or
none once it has ended). Until a line starts, the answer waits 2 s; once one starts,
it waits for that line's seconds left plus 2 s. A wait that runs out answers with
the state as it stands, noting "stopped waiting after N s; the conversation may
still be moving". Stop and quit still run at once.

A line cannot be skipped early: a call made during the line waits for it, as any
verb does. Calls made during speech queue in order behind the held call and run in
turn, each checked again when it runs. stop does not clear the queue: a call held
there still runs when the speech ends.

The wait for a held call comes from the game. For a line already started, `wait_s`
in the interim frame is the same value that info reports as `hold_left_s` (seconds
left plus 2). Each call with a limit (key_press and the goal calls) also gets 5 s of
slack on top of its limit, so the reply still comes after a refusal at the cap.
choose_reply and continue_dialog have no limit on the server side: their answer
comes when the game gives it, so no margin applies to them. stop, quit and
game_status are never held and get no slack. info also reports `speaking`,
`speaking_left_s` and `speaking_total_s`.

A call that waited carries `waited_s` (seconds) in its data, and the reply shows its
wait message first, before the data. The game's clock keeps running during speech
(unless the game is paused), so the goal watch does not count it as a stall.

## Open windows

Something on screen can take the input: a container window, another window (for
example inventory, character, journal, map, a store, the pause menu), a
conversation or a message box, the level-up screen, the game over screen, a movie,
a module that is loading, the game paused, or no game running. A semantic verb
that the screen does not take is refused at once, before it is queued, and the
reply names what is open and how to close it, for example "refused: the
container window 'Corpse' is open; close it with take_all, or key_press Escape".

What each screen takes: a conversation takes choose_reply and continue_dialog; a
message box takes continue_dialog; a container window takes take_item and
take_all; the level-up screen takes level_up; another window takes equip, unequip
and use_item (not the pause menu). The pause menu, a movie, loading, the game over
screen and a paused game take none of them. Reads, host verbs (keys, pointer, pick,
project, screenshot) and stop, quit, save and load run whatever is open.

Escape closes the open window (key_press Escape), and a movie is skipped the same
way. A paused game is resumed with key_press Space. The level-up screen is finished
with level_up, or Escape. take_all closes the container window when it answers at
once (the leader already in reach). take_item closes it when that take leaves the
container empty. A take that walks first leaves the window open. The game over
screen takes no game verb; only reads, host verbs, stop, quit, save and load run
there.

observe puts the open window first: "open: container window 'Corpse' (2 items)",
and the same line is in its data as `open`. Raw input (key_press, type_text,
mouse_move, mouse_click, mouse_drag, scroll) is delivered even while a window is
open, and the reply then says "input went to the open <name> window", for example
the container or the pause menu.

## Goals that do not advance

A goal call (move_to, interact, talk_to, transition, wait_until) is watched. The
server reads the game's world time once a second while it waits. If the time
stands still for 20 s, the game is paused, in a movie or on a loading screen,
and the server stops the goal and answers "game not advancing (phase ...)" with
the phase. The game keeps running. A goal that is still going after 300 s of
wall clock time (plus 5 s of slack and any speech hold) is stopped the same way,
and the answer says "the goal did not finish within 305 s of wall clock time and
was stopped". take_item, take_all and use_skill are goals too, but they are not
watched and have no wall-clock cap: their 60 game-second limit runs only while game
time advances, so a paused game leaves such a call waiting.

## Cutscenes

A cutscene holds the leader: a movie, a conversation (other than a message box),
input blocked for a moment, a leader who is dead, down or not commandable, or a
scripted action on the leader other than an attack. move_to, interact, talk_to,
use_skill, take_item, take_all and transition are refused at once while one holds
the leader ("refused: a cutscene is running"). A walk already under way stops where
the leader stands when a cutscene starts, and answers "interrupted: a cutscene
started". Two things are not interrupted: an exit that has already taken the leader
into another module, and wait_until. attack, use_power, use_item, equip, unequip,
switch_leader, set_party and level_up are not held.

## Movement arrival

`move_to` takes an object id only: the leader walks to it, the way a click does,
round walls and closed doors, and never teleports. `run` true runs instead of
walking, and `timeout` (game seconds, default 60) is its limit. It stops within the
object's reach: 1.8 m plus the object's radius. The radius is 1.2 for a usable door
(a static door only with a conversation), 0.8 for a usable placeable, the
creature's radius (at least 0.4) for a creature other than the leader that is
neither dead nor down (or is a container), and 0.5 for everything else. That gives
3.0 m for a door, 2.6 m for a placeable, 2.2 m at least for a creature and 2.3 m
for other objects. The walk that `interact` and `talk_to` make first stops at the
same reach. Walking to a position is not offered over MCP; to walk a distance, use
key_press.

## Walk, then act

`interact`, `talk_to`, `take_item`, `take_all` and `use_skill` walk the leader
first, as a click does. A key in the party's inventory is used where the leader
stands. `use_item` and `use_power` do not walk first. A search finds a spot 0.5 m
inside reach of the object, one search step a frame, and the leader is steered
there. A walk that gets within 0.5 m of its spot is in reach, so the arrival test is
the same reach test the verb uses. The verb acts once the leader stands in reach,
and the reply comes with the act. Nothing is used before that. If no spot in reach
can be walked to, or the walk gets no nearer its aim for 1.5 s, the reply is
"Unable to reach target!" and the object is left alone. These walks give up after
60 game seconds. take_item and take_all open the container first when it is not
open. `transition` walks the same way: to a door in reach, or to the
middle of an exit trigger. `attack` is not part of this: it answers at once, and the
combat approach walks.

A target with no stand in reach at all (no route to one) answers at once, with
no movement. A route never crosses an enabled blocker: a wall, a placeable, or a
static closed door. A closed door that is not static is passable for routes at a
cost: a little when unlocked, a lot when locked, so a route goes round a locked
door when it can. A walker in reach of an unlocked closed door opens it (the use
path, so a door with a conversation talks and the walk ends there), then waits for
the swing and walks on. A walker in reach of a locked door stops there: the use
gives the locked-door feedback and runs OnFailToOpen, and the walk ends with "... is
locked". The control walks (move_to, interact, talk_to, take, use_skill) and NPC
movers and the click walk do this. A closed door's doorway has no walkable
faces, so the grid joins the floor either side of each door (a doorway link), and
the route search counts passing a closed door as a cost (2 m unlocked, 40 m
locked). The last leg of a steering route to its destination is a straight line only
when no blocker is on it.

The reach test lives in the control layer. World::use_object does not check
reach, so scripts and other World callers keep their own rules.

`wait_until idle` answers once the party is in control and settled for 0.5 game
seconds, with nothing pending. Pending means: a DelayCommand or other script
resume that holds the party, a queued script event, a module change, a movie,
input blocked for a moment, or a queued action on a party member other than a
follow. A companion's heartbeat follow stays queued and counts as idle. The
exception is an area loop: a resume from an area script that has already queued its
own next resume
(the ambient loop that runs for the whole visit). A one-shot delay from an area
script holds the party, unless that script is already a loop on the same area:
then its key makes every resume of it ambient, one-shot or not. A loop is kept
per area and script, and it ends with the module change or the turn its script
stops re-arming. A two-script loop (A queues B, and B queues A) never reads as a
loop, because each script re-arms the other rather than itself, so its resumes
hold the party. The loop is learnt at runtime and is not saved, so after a load
the first pending resume of an area loop holds the party until the loop's next
turn. A finite chain of an area script's own delays reads as a loop until its
last turn, since the rule cannot tell a chain from a loop before the chain ends.
`wait_until dialog` answers at once while a conversation runs, and `area_loaded`
answers when the module has changed. A wait that nothing pending (the list above)
could bring about is refused at once with the reason ("refused: ..."). Idle is also
refused while a conversation runs, and every wait while the game is paused.
Otherwise it waits for `timeout` game seconds: 1 by default, at most 30, and then
answers "timed out after N s waiting for Idle" (or Dialog, AreaLoaded). A cutscene
does not interrupt a wait.

## Objects and dialogs

observe marks a nearby object `scripted` (its data has `scripted_use`) when the
object has an OnUsed script. Using such an object may open a dialog that observe
does not show. `interact` refuses a static door, and a placeable that cannot be
used, with "cannot be used".

## Input

Keys are named as Bevy's KeyCode: `KeyA`..`KeyZ` (or `a`..`z`), `Digit0`..`9`,
`F1`..`F12`, `Enter`, `Space`, `Escape`, `Tab`, `Backspace`, `Delete`,
`ArrowUp` (and the other arrows), `ShiftLeft`, `ControlLeft`, `AltLeft`,
`SuperLeft`, and common punctuation names. Modifiers: `shift`, `ctrl`, `alt`,
`super`. A printable key (letter, digit, space) also types its character,
unless ctrl or super is held. `Escape` skips a playing movie.

`key_press` presses the modifiers and the key, holds the key for `hold_ms` (default
60) of game time, then releases the key and the modifiers. The hold runs on the
game clock, so a frame over the clamp does not shorten it. The call answers when
the key is up, with `held_s` (game seconds held), `moved_m` (the leader's
horizontal distance from where the press began) and `position`. A paused game, a
conversation, a movie, loading or a game out of play refuses the press at once
("key_press refused: the game is paused; nothing was pressed", or "is in a
conversation", "is showing a movie", "is loading", "is not in the game"). If the
game stops while the key is down, the key comes up at once and the answer says so. A game clock that stands
still releases the key at the wall-clock limit (10 s plus three times the hold).
`type_text` presses each character with its text and releases it (capitals
hold shift; newline is Enter; other characters are refused). At most 200
characters, at most 1000 ms between characters.

Holds are capped: `hold_ms` at most 10000 (key_press and mouse_click), and a
drag passes through at most 300 positions. Larger values are clamped, not
refused. A plan has a deadline of 30 s, or its own length plus 5 s when that is
longer. If it passes with events still pending (no window took them), the
request fails, the pending events are dropped, and every key and button the
window holds is released. `stop` also releases every key and button the window
holds, whether or not an input job is running.

Pointer positions (mouse_move, mouse_click, mouse_drag, scroll with a position,
pick) must be finite and inside the window, or the call is refused.

Mouse positions are logical window pixels, origin top left. The screenshot is
in physical pixels, so `logical = physical / scale_factor`. Info's `window`
carries `width` and `height` (logical), `physical_width` and `physical_height`,
`scale_factor`, and `settled`. `settled` is true after 10 frames with an
unchanged physical size and scale. The `screenshot` reply gives the PNG's
`width` and `height` (physical pixels) and `scale_factor`. `pick` and `project`
take and return logical positions. `project` gives x and y as null when the
object is behind the camera, with on_screen false.

`mouse_click` moves, then presses and releases (`clicks: 2` for a double click).
`mouse_drag` presses at `from`, moves in `steps`, releases at `to`. `scroll`
with positive `lines_y` scrolls up.

Injected events are Bevy's own input messages (KeyboardInput, CursorMoved,
MouseButtonInput, MouseWheel). The window's cursor position is updated too,
since the game reads it directly. Input therefore reaches the same input
systems, picking, GUI hit-testing and egui as a player's. Nothing leaves the
game process: no desktop-level injection (no wtype, ydotool or xdotool).

A request that delivers input answers when every event has been written, in
under a second for ordinary use. Not simulated: IME composition, touchpad
gestures, pressure, and keyboard repeat.

## Screenshots, traces and logs

`screenshot` captures the window's rendered frame with Bevy's screenshot API,
saves a PNG under the run's `shots/`, and returns it as an image block with its
`path`, `width`, `height` and `scale_factor`. The image is read only from the
run's `shots/` folder: a reply that names a file elsewhere is refused and no
image is returned. It fails after 15 s if no frame arrives (for example, the
window is on a hidden workspace). A downscale parameter is not implemented. Its
`path` is a bare file name under `shots/`, or a path inside it. An existing file
is refused, and nothing is overwritten.

`trace_start(path, filter)` starts a JSON trace in the run's `traces/` folder.
`path` takes the same rules as the screenshot path (a bare name or a path
inside `traces/`, an existing file refused). `filter` is the trace's own
EnvFilter, default `info,kotor2=trace,k2_world=trace`; the console filter does
not change. The file holds one object per event and a close event per span
(busy and idle times). The trace can start and stop any number of times while
the game runs. `trace_stop` closes the file and returns its `path`, or says that
no trace was running. The format is JSON span log, not Chrome trace.

Game output (stdout and stderr) goes into a ring of the newest 20,000 lines,
kept to 32 MiB of text. The oldest lines go first past either limit, and the
cursor reports the gap as dropped. The run's log file is capped at 64 MiB: past
that it gets one marker line and no more, and the ring keeps the newest output. Each line keeps
its first 16 KiB; a longer line is cut and says how many bytes were cut. Bytes
that are not UTF-8 are shown with replacement characters. The reader stops only
at the end of the stream or on a read error.

`logs_tail(n, filter, since)` returns lines oldest first. `filter` is a regular
expression over the line text. `since` is the cursor of a previous reply, and
it returns the lines from there on; a page that stops early resumes after its
last line. A cursor belongs to one run. A cursor from an earlier run returns
the newest lines of the current run with `dropped` true. `dropped` is also true
when the cursor is older than the ring.

Log levels: the game does not use Bevy's LogPlugin (its filter is fixed at
startup). `logging.rs` installs the subscriber with Bevy's default filter plus
RUST_LOG, and `set_log_filter` replaces the console filter while the game runs.
The trace has its own filter (`trace_start`). A bad filter is refused and the
old one stays.

Crashes and exits: the supervisor records how each run ended. `stopped` is a
`game_stop` (or the server shutting down). `exited_on_its_own` is an exit with
code 0 that nobody asked for, such as the player quitting the game. `crashed`
is any other exit (a non-zero code or a signal, for example SIGSEGV). For a
crash, `game_status` shows it with the last 40 output lines, and every call made
while no game runs returns that report as an error, until the next
`game_start`. After an exit on its own, calls say so and `game_start` starts a
new game.

## Registering with Claude Code

    claude mcp add k2 -- $HOME/.local/share/Steam/steamapps/common/Knights\ of\ the\ Old\ Republic\ II/kotor2-rs/target/release/k2-mcp

The server needs no arguments. The checkout's path is compiled in.

## Safety

- The game runs in a window with its own saves, settings, control socket,
  screenshots, traces and mods under the run root, and that isolation is
  enforced, not just the default. `game_start` refuses extra flags outside the
  allowlist and environment names that would point the game's folders
  elsewhere (see Launching). The run root cannot reach the player's data folder,
  even through a link.
- The editor is not started through the MCP: `--edit` and the editor flags are
  refused. The game reads loose mod folders only from K2_MOD_DIR (the run's own
  `mods/`, empty) and from `--mod-dir`, which is refused. No player mod folder is
  read. The editor writes its mod capsule into the first K2_MOD_DIR folder, which
  is the run's `mods/`, never the player's `~/.local/share/kotor2-rs/mods`.
- Headless entry points (surveys, the editor, screenshot runs, the audio test)
  are not reachable through the MCP.
- Saves written through the control API use `mcp-` slot names only, so the
  player's `save-NNNN`, `autosave`, `quicksave`, `restart` and `timed` slots
  are never overwritten by an agent. `load` can read any slot in the game's
  save folder: it only reads, and it replaces the game's state.
- The control socket is created with mode 0600 (owner only). Anyone running as
  the same user can drive the game; there is no other authentication.
- No desktop input is injected. Input goes only into the game's own event
  queue.
- The game process is started by k2-mcp and stopped by it. The supervisor
  never signals a process it did not start. Old build snapshots go to the trash
  with trash-put (rebuildable output, not the player's files).
- `K2_MCP_GAME_BIN` (run another binary instead of a build) is honoured only in
  tests (`K2_MCP_TEST=1` or the unit tests). Outside that it is ignored, with a
  warning on stderr.
- `debug_*` verbs change state without the rules. They are offered only with
  `K2_MCP_DEBUG=1` on the server, and they are for tests.
- The server reads a screenshot's image only from `shots/`.

Known limits (accepted):

- The parent-death signal is SIGTERM, with no SIGKILL fallback. A game that
  ignores SIGTERM after its server was SIGKILLed keeps running until the user
  ends it.
- Environment names other than the refused ones pass through to the game (for
  example RUST_LOG, DISPLAY, XAUTHORITY, PATH). They can change logging and the
  window's session, not where the game keeps its data.
- A build cut short by a killed server (SIGKILL) is not stopped; it finishes on
  its own and starts nothing.

## Tests

Run from kotor2-rs. None opens a window. Each test that needs a game uses a
stand-in binary (`k2-mcp-fake-game`) through `K2_MCP_GAME_BIN` (with
`K2_MCP_TEST=1`), which answers the control socket and exits on quit. Every
wait on a server process is bounded: a server still running after 30 s is
killed and the test fails, and a response that takes longer than 60 s fails the
test.

- `cargo test --release -p k2-mcp --bins`: log ring (cursors and their runs,
  the 20,000-line and 32 MiB budgets, the 64 MiB file cap), the capped line
  reader, regex filter, run folder and UTC stamps, owner-only folders, the run
  root refusal (lexical, through links, a stand-in player folder, a colon),
  create_all under the player folder changing nothing, a link never chmodded, a
  root without the layout marker or with a file named like a layout folder
  refused (nothing created), a link in the tree leaving no folders behind, a
  writable folder in the tree refused and left alone, a symlinked ancestor
  accepted, a parent writable by others refused, a grandparent writable by
  others refused, missing parents created owner-only, a second server refused by
  the lock, the argument and environment policy (names with `=` or NUL, case),
  snapshot pruning (through a stand-in for trash-put),
  the start flags, the settled window, the closing gate, a live socket left
  alone and a stale one cleared, a green build snapshotted into an owner-only
  folder, a red build reported, and a build stopped by kill_build (a scratch
  crate with a slow build script).
- `cargo test --release -p k2-mcp --test mcp`: over stdio: handshake and tool
  listing (debug verbs hidden, advance_time and new_game gone), the no-game
  messages; debug verbs offered with `K2_MCP_DEBUG=1`; refused arguments and
  starts (nothing launches); K2_MCP_GAME_BIN needs test mode; a run root inside
  the player folder, and one reached through a link, are refused with the player
  folder's mode unchanged; two servers on one run root (the second refused); a
  supervised fake game (start, status, observe, key press, screenshot as an
  image block, pick, project, trace filter, logs with a filter, log filter, the
  chosen start flags reach the game, the game's mod folder is the run's own,
  owner-only folders, stop); a screenshot outside shots/ is not read; a window
  that never settles (no size in the reply); a goal whose game stands still is
  stopped with the reason; a fake game that crashes (exit code and last output
  reported on every later call); a fake game that quits on its own (not a
  crash); a log cursor from an earlier run; start and stop, two starts, and
  restart and start racing; a restart's order (save, then stop, then relaunch
  from the save), and a restart whose save is refused (the game keeps running);
  SIGKILL of the server (the game exits); SIGTERM of
  the server (the game is stopped, the server exits cleanly).
- `cargo test --release -p kotor2 -- remote remote_input logging`: input plans and key
  names; delivery on a headless Bevy App with MinimalPlugins and InputPlugin
  (key state and window cursor follow the injected events); the filter and the
  trace layer changing while the subscriber runs.
- `cargo test --release -p k2-world --lib control` and `--test control`,
  `--test control_socket`: the command set on the wire, the semantic verbs in
  the prologue and Peragus against the real install, the socket driven frame by
  frame, host verbs refused by the world layer.

## Unverified

Nothing here has been run against a live game window. Live checks, which need
the user's go:

1. `game_start` with `start: "menu"` (or `"new"`), then `game_status` (expect
   window.settled true, phase menu or game, and the window size).
2. `game_start` with `start: "chargen"`: expect character creation to open from
   the main menu (the `--chargen` flag).
3. `screenshot` with workspace 4 visible (expect an image of the game, and
   width and height equal to the logical size times scale_factor).
4. `pick` on a visible button (object id, name and kind, or null), and `project`
   on a nearby object id from observe (x and y on screen).
5. `mouse_click` on a visible button, `key_press` on `Escape`, `type_text` in a
   text field, then `observe` to see the result.
6. `set_log_filter` to `debug`, then `logs_tail` with a filter; `trace_start`,
   a few actions, `trace_stop`, and read the file.
7. `game_stop`, then `game_status` (expect the expected exit and a save in
   `saves/`).
8. Kill the server (SIGKILL) while a window is open; expect the window to close
   and the game to save.
9. `game_restart` while the game is playing: expect the progress made during the
   build to be in the restart save.

Known gaps: `rest` is not in the API (the engine has no rest routine).
`trapped` in observe is set for armed traps and mines only. Live `load`
returns once queued. `game_start` can take minutes when it builds, so the MCP
client's tool timeout must allow for that. Game-side paths that depend on a
window (screenshot, cursor, egui under injected input) are untested until the
live checks above run.

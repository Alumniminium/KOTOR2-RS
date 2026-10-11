# Save games

Our own format, not the original SAV/ERF layout. Since format 3 the world
state is CBOR with struct fields keyed by name, so saves survive the
schema changes that happen constantly during development (see "Adding
saved fields"). Format 2 (positional postcard) saves are refused with
"saved by an older incompatible build" rather than misread.

## On disk

`$XDG_DATA_HOME/kotor2-rs/saves/<slot>/` (default
`~/.local/share/kotor2-rs/saves/`), never inside the game folder.
`K2_SAVE_DIR` overrides the root. Scripted runs (`--screenshot`) default
to `$TMPDIR/kotor2-rs-test-saves`, so test autosaves never overwrite the
player's.

- `save.bin`: `K2RSAVE\0` magic, u32 LE format version
  (`SAVE_VERSION` = 3), u32 LE module name length, module name, then the
  world state as CBOR (ciborium). Written to `save.bin.tmp` then renamed,
  so a crash cannot leave a half-written save. The Load screen checks the
  16-byte header only (`save::slot_compatibility`) and marks slots it
  cannot read "(incompatible)", with the reason on the details panel;
  pressing Load on one plays the error sound and does nothing.
- `meta.json`: `SaveMeta` (name, module, area name, play time, unix
  timestamp, party as (name, appearance row)). The load screen reads only
  this and the thumbnail.
- `thumb.png`: 256x144, captured one frame after saving with all UI
  hidden.

Slots: `quicksave` (F5/F9), `autosave` (module entry), `save-NNNN` (named
saves from the Save Game screen). Delete moves a slot to
`<root>/.deleted/<slot>-<unix time>` instead of removing it.

Sizes: avg ~118 KB per save across the 82 modules (postcard was ~55 KB;
field names cost ~2.1x). Save+load including the world rebuild: avg 59 ms,
max 285 ms (k2world saves).

## Adding saved fields (rule for every agent)

- A new field on a saved struct must load from saves that lack it. Structs
  that derive or implement Default carry container-level
  `#[serde(default)]` (all of them in k2-world and k2-script as of format
  3), so new fields there just work. Structs without Default (Object,
  Timer, ModuleSnapshot, Round, CraftScreen, Effect, Talent,
  AppliedEffect, CastAction, ScriptState, EngineValue) need
  `#[serde(default)]` on the new field itself (its type must be Default),
  or `#[serde(default = "fn")]`.
- New saved structs: derive Default if possible and add
  `#[serde(default)]` to the container.
- Removing a field is safe: unknown keys are ignored. Never add
  `#[serde(deny_unknown_fields)]`.
- Renaming a field loses its value in old saves; use
  `#[serde(alias = "old_name")]`.
- Enum variants are keyed by name: adding variants is safe; renaming or
  removing a variant breaks only saves that contain it (add
  `#[serde(alias)]` on the variant when renaming).
- Bump `SAVE_VERSION` only for a container change the CBOR layer cannot
  absorb.
- `k2world saves` (all modules round-trip) and
  `cargo test -p k2-world --test save --test save_evolution` must pass.

## What is saved

Everything needed to resume exactly: all objects (positions, HP, locals,
action queues, inventories, door/placeable state, created and destroyed
objects), globals (numbers, booleans, strings, locations), party and
available NPCs, the stashed PC (SwitchPlayerCharacter), the player spec,
journal states and update times, tokens, influence, galaxy map state, game
time, timers (DelayCommand states), the engine-structure store (effects,
events, locations, talents: script values hold handles into it), the
script RNG, and the subsystem state of combat (factions incl. overrides,
rounds in flight, per-creature memory, its RNG), paths, crowd, stores,
crafting (skills, installed upgrades, crystal overrides, RNG),
progression (known feats/powers, level-ups, journal XP paid), powers
(last power/feat, effect -> spell links, absorb pools, bonus force; the
live cast stack is transient) and module memory. Effect handles in powers
and on objects index the saved engine store, so numbering is preserved.

Rebuilt on load instead of saved: resources, tables, TLK, compiled
scripts, walkmesh and door/placeable blockers (from the saved door state),
item definitions and other lazily filled caches.

Deferred script states (`ScriptState`) carry their script as `Arc<Ncs>`.
They serialize the script name only (`k2_script::saved`). Loading runs the
decode with a name table of compiled scripts; names not yet loaded come
back as placeholders and are reported, the world loads them (the save's
module is mounted first) and decodes again.

`Action::Noop` holds a `Cow<'static, str>` (was `&'static str`, which serde
cannot deserialize without leaking).

## DoorState.locked_at_load

`locked_at_load: Option<bool>` (k2-world object.rs) is the door's Locked as its
area loaded: spawn.rs sets it from the blueprint and it is saved with the door.
Saves from before the field have None; `World::load` (save.rs) fills it from
the door template's Locked, and a template that fails to load counts as
unlocked. lib.rs use_object reads it: a door locked at load that a script has
since unlocked opens on use instead of talking.

## When saving is allowed

`World::save_blocked()` returns why not: no module, in conversation,
changing area, in a minigame or pazaak, scripts running, or a
container/store/craft screen open. Combat does not block saving (it used
to): F5, autosave, the Save Game screen and the ctl `save` command all
work mid-fight. Conversations are never part of a save. `World::save()`
itself does not check (the tests save mid-fight).

A mid-fight save keeps everything combat needs: rounds in flight, attack
queues, per-creature combat memory (so `in_combat` stays true), AI timers,
HP, deaths and the combat RNG. `saving_is_allowed_mid_fight_and_restores_the_fight`
(tests/save.rs) fights in 102per, checks that saving is never refused by
combat, then saves 2 s into the fight with attacks queued and compares HP,
deaths, in-combat flags, positions, action queues and the digest between
the live and the loaded world; `save_round_trip_mid_combat_is_deterministic`
then runs both worlds for 10 s with identical input and the digests stay
equal.

## Module memory

Leaving a module snapshots its objects (everything except the party and
the party's items, which carry over) into `World::module_memory`, with
action queues and trigger occupancy cleared, plus its stores, kept corpses
and AI styles. Re-entering restores them with their ids instead of
spawning the GIT: the dead stay dead, opened doors stay open, looted
containers stay empty. OnSpawn and Mod_OnModStart do not run again;
Mod_OnModLoad, Mod_OnClientEntr and the area OnEnter do (as in the
original, scripts guard their one-time work with locals). Module memory is
part of the save.

## Determinism tests

`crates/k2-world/tests/save.rs`:

- 102per, 30 s with a fight, save, load into a fresh World, then run both
  10 s with identical input: state digests equal every second.
- Same, saved 2 s into the fight (36 hostiles alive, rounds and AI timers
  in flight). Control: reseeding the loaded world's combat RNG makes the
  comparison fail, so the digest does catch divergence.
- Kill the 102per droids, go to 101per and back: same objects, still dead.

`World::state_digest()` hashes objects, globals, party, time and the RNG
in a HashMap-order-independent way.

`k2world saves [MODULE...] [--seconds N]` does the round trip for every
module: run N s (default 10), wait until saving is allowed, save, load,
run both 5 s comparing digests each second. Result: 79 of 82 saved and
identical; 3 skipped because the harness never reaches a saveable moment
(005ebo and 907mal wait on a transition the harness does not take, 403dxn
loops a conversation). `K2_SAVE_FRAMES=1` compares every frame instead;
`K2_SAVE_TRACE=N` traces scripts of both worlds from step N (how the one
divergence found, a conversation not being part of the save, was located).

## In the game (kotor2, `src/saves.rs`)

- F5 quick save, F9 quick load. Feedback as a floating line over the
  leader ("Game saved.", "Cannot save: in conversation.").
- Options menu (Esc): Save Game and Load Game open the game's own
  `saveload` screen: slot list (New Slot first when saving), thumbnail,
  PC name, area, planet (from the module suffix), time played, party
  portraits (portraits.2da via the HUD lookup). Save asks for a name on
  the `savename` screen; typing is captured so menu shortcuts do not fire.
- Main menu: Load Game opens the same screen over the menu.
- Timed autosave (`timed_autosave`, `saves::TIMED_SLOT`): every 120 s of
  real time while in game, into its own slot `timed` ("Timed Auto Save"),
  so autosave, quicksave and restart stay intact. Same `save_blocked`
  rules as a normal save (conversation, area change, scripts, open
  menu, area loading): a blocked save is retried 10 s later. Logs `save:
  timed autosave timed in ... -> DIR` (or `skipped: WHY`). Written at once
  without a thumbnail (no UI flicker, about 2 ms); not offered as a target in
  the Save Game list. `--continue` includes it in the newest-by-mtime choice.
  Tests: `cargo test --release -p kotor2 -- timed`.
- Autosave to `autosave` on each module entry once saving is allowed
  (within 30 s), not after loading a save.
- Loading builds a fresh World from the slot (`World::load`), swaps it in
  at the end of the frame, resets transition/movie/menu/pause state and
  reloads the area like a transition (fade in).

Test hooks: `K2_SAVE_TEST=quick` (quick save 3 s in, quick load 3 s
later, log both digests and PASS/FAIL), `K2_SAVE_OPEN=load|save` (open the
screen once).

## Control channel and --continue (`crates/kotor2/src/ctl.rs`)

For a shell driving the game (the player plays, an agent rebuilds and
restarts). Control dir: `K2_CTL_DIR`, default
`~/.local/share/kotor2-rs/ctl`; `--screenshot` runs have none unless
`K2_CTL_DIR` is set. The game writes `pid` at startup (removed on exit),
deletes any stale `cmd`/`ack` left by an earlier run, polls `cmd` every
0.2 s (no window focus needed) and consumes it. Commands, one per line,
appended by the caller:

- `save`: full save into slot `restart` (the only slot it ever writes),
  plus `restart/digest.txt` with the state digest. Waits up to 10 s for
  saving to be allowed (scripts running, a menu open, ...).
- `quit`: save as above, then exit cleanly. `quit now`: exit, no save.
- `load SLOT`: load a slot. `save` and `load` in a menu or movie answer
  `err ... not in game`.

The answer is one line in `ack` (written atomically, last command wins):
`ok save slot=restart ts=UNIX digest=0x...`, `err save not in game ts=...`,
`ok load slot=...`. The ack for `save` goes out once the thumbnail is
written (or after 3 s).

`kotor2 --continue` skips logos, movies and the main menu and loads the
newest readable slot, by `save.bin` mtime, among `restart`, `timed`, `autosave`,
`quicksave` and `save-NNNN` (restart wins a tie); `--load-slot NAME` loads
that slot. Log lines: `ctl: saved restart: module M leader at (x, y, z)
digest D` on save and, after loading, `continue: PASS slot restart loaded
digest D matches the saved one; module M leader at (x, y, z)`. A digest
that differs from the one in `digest.txt` is only a WARN, `continue: loaded
(digest differs, engine changed) ...`: a new build with new state fields
hashes the same save differently. Real load failures stay ERROR
(`load SLOT: ...`). Module, party, positions, inventory, time, journal and
script state come from the normal save.

Save on exit: on every normal exit while in game (window close, Esc menu
Quit, SIGTERM/SIGINT/SIGHUP, any AppExit) the game writes slot `restart`
and logs `ctl: saved restart on exit: ...` (or `ctl: not saved on exit:
WHY`), so closing the window never loses progress. It is the same save as
the `save` command but written at once without a thumbnail (the old
`thumb.png` is removed). A conversation does not block it
(`World::exit_save_blocked`; conversations are not part of a save, the
loaded world resumes without it); an area change, minigame, pazaak, running
scripts or an open container/store/craft screen do, and then the slot keeps
its older state (`--continue` still takes the newest slot). Not done from
menus, for `--screenshot` or `--edit` runs, after `quit` (already saved) or
`quit now`. Tests: `cargo test --release -p kotor2 ctl::` (slot choice,
exit save round trip, SIGTERM) and `exit_save_in_a_conversation_loads_cleanly`
in `crates/k2-world/tests/save.rs`.

`tools/restart.sh [--build] [--force] [-- GAME_ARGS]` (copy at
`/tmp/k2research/play/restart.sh`) does the whole cycle: optional
`cargo build --release` (a red build aborts with the game untouched; a
green one is snapshotted atomically to `/tmp/k2research/play/kotor2`),
`save`, wait for the ack, `quit now`, SIGTERM (which also saves, as above) then SIGKILL for the pid in
the pid file only if the save or quit fails, and a detached relaunch
`WAYLAND_DISPLAY= kotor2 --install <root> --continue` logging to
`/tmp/k2research/play/session.log` (the old log becomes `session.log.prev`).
If the save fails, the stale `restart` slot is moved to `.deleted/` so
`--continue` takes the newest autosave. A game with no pid file (started
before this existed) is never touched and no second one is started
beside it unless `--force`. Overrides for tests: `K2_CTL_DIR`,
`K2_SAVE_DIR`, `K2_PLAY_BIN`, `K2_PLAY_DIR`, `K2_SESSION_LOG`.

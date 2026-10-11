# Area editor

`kotor2 --edit [MODULE] [--mod-name NAME]`, or F10 in game. Without a module the
editor starts on the Load Map list (over 101per, the default start area). Code:
`crates/kotor2/src/editor/` (`doc.rs` is the Bevy-free document, `mod.rs` the
state, camera, picking, handles and views, `ui.rs` the egui user interface).

## Model

- The editor edits a document, not the live world. On entering, the world
  pauses (`PauseState::editor`), its objects are hidden, and the area's GIT
  is loaded into `EditDoc`. Each GIT list entry becomes an `Inst` holding
  its original `GffStruct`, so fields the editor doesn't know survive
  untouched. Saving rebuilds the root's lists from the instances in order.
- Leaving the editor drops the document and shows the world again. Edits
  reach the game only through a saved mod folder.
- Positions: doors and placeables use X/Y/Z + Bearing, cameras a Position
  vector, everything else XPosition/YPosition/ZPosition +
  XOrientation/YOrientation. Trigger and encounter geometry points are
  relative to the entry's position (checked against
  `TriggerInstance::world_polygon`).
- Tags: doors, waypoints, triggers and stores keep Tag in the GIT; creatures,
  placeables, sounds and encounters take it from the blueprint.
- Blueprint fields (conversation, scripts, flags, creature tag): the first
  edit forks the blueprint for this instance only. The fork is a copy named
  `<template[..12]>_eNN`, with its TemplateResRef set, and the instance
  points at it. Other instances of the template are unchanged. A fork used
  by only one instance is edited in place afterwards.
- Undo/redo: whole-document snapshots (instances + forks), 200 deep.
- New instances clone an existing GIT entry of the same kind (so every field
  the format carries is present). They fall back to a minimal field set when
  the area has none, and reset LinkedTo/LinkedToModule on the clone.

## Output

- `~/.local/share/kotor2-rs/mods/<name>.mod` (default name `editor`), a MOD
  capsule ("MOD V1.0", ERF family, written by rakata's `write_erf_to_vec`
  through `k2_res::write_mod`). It holds `<area>.git` plus `<fork>.ut?`
  entries. Written to `<name>.mod.tmp` and renamed, so a failed save keeps
  the previous capsule. Never the game folder.
- Playing: `kotor2 --mod-dir PATH` (repeatable), or `K2_MOD_DIR=PATH` for
  every tool, k2world included. PATH is a .mod capsule or a folder of loose
  files (override layout). k2-res searches `Install::mod_dirs` ahead of
  override/, first entry wins across both kinds (`mount_overrides`), then
  follows the usual order. A capsule that fails to open is skipped with a
  warning on stderr, not an error. Loose folders under mods/<name>/ load only
  when named with --mod-dir (or K2_MOD_DIR); nothing scans mods/.
- Area names: the Load Map list shows the entry area's ARE `Name` through the
  TLK, else that area's name in the module named after it (a copy can be a
  placeholder: 006ebo's entry area 003ebo is "Untitled" there), else the IFO
  `Mod_Name`, else the resref (`doc::map_name`; `map_title` is the same
  without the resref fallback). Read with `Install::read_module_resource`, so
  no module is mounted. A placeholder is skipped (`is_placeholder`): the
  module's or entry area's resref in any case ("101PER", 907mal's "904MAL"),
  "Untitled", or "module" plus digits. Other names are real, one word included.
  Filtering matches both the resref and the name.

## User interface (egui)

The UI is immediate mode: egui 0.36 through bevy_egui 0.42 (the newest stable
bevy_egui line for Bevy 0.19; 0.43 is rc and tracks Bevy main). It replaced
the first version, which was hand-built Bevy UI nodes with a text-only
inspector. `ui.rs` draws everything from one system in
`EguiPrimaryContextPass`.

- The plugin is registered once (`EditorPlugin`) with
  `auto_create_primary_context = false`. The egui context
  (`PrimaryEguiContext`) is put on the 3D camera when the editor opens and
  removed when it closes, so in play egui has no context and its systems
  run over empty queries. The game HUD is untouched.
- Menu bar. File: Load Map (Ctrl+O), Save mod (Ctrl+S), Revert (asks first),
  Exit editor (F10). Edit: Undo/Redo (Ctrl+Z/Y), Delete, Duplicate (Ctrl+D).
  View: a checkbox per kind (hides that kind's views, outlines and picking),
  markers, move/rotate handles, grid, walkmesh overlay, palette (Tab).
- Left panel: object tree by kind with counts, a filter box (template, tag,
  kind), click to select, double click to fly the camera there. Selecting in
  the view scrolls the tree to the row and opens its group.
- Right panel: inspector. Template (text plus drop-down of every blueprint of
  the kind; swaps the blueprint), drag values for X/Y/Z and bearing, then one
  widget per field: checkbox for flags (Plot, Locked, Useable, Active), drag
  value for FactionID, text fields for Tag/LinkedTo, text plus filtered
  drop-down for conversations (DLG), script slots (NCS) and LinkedToModule.
  Each field shows `git` or `bp` (where it lives). A line under the template
  says whether the blueprint is shared or a fork used only by this object.
  Typed edits are one undo step each; a drag or a checkbox burst is one step.
- Bottom bar: module, mod name, unsaved marker, tool, selection, ground point
  under the cursor, last status message.
- Palette window (Tab): kind filter, search, templates with names from the
  TLK; click one, then click the walkmesh to place (Esc cancels). Enter in
  the search box picks the first match. Triggers and encounters: click the
  polygon points, Enter finishes (3+ points).
- Load Map modal (Ctrl+O): type to filter, Up/Down/PageUp/PageDown move,
  Enter or a click opens, Esc closes. Loading over unsaved edits shows
  Discard and load / Save and load / Cancel (Esc cancels too).
- Move/rotate handles on the selection: red/green/blue arrows for X/Y/Z,
  an orange ring with a knob for the bearing (creatures, doors, placeables,
  waypoints). Dragging an arrow slides along that axis (the ray is
  intersected with the axis line); dragging the knob turns the object toward
  the cursor on the ground plane. Handles draw on top (`HandleGizmos`, depth
  bias) and scale with distance. A hovered or dragged handle turns yellow.
- Input: the 3D view ignores the pointer over a panel, window, modal or open
  popup (`ui_rects` from the panel and window responses, plus
  `egui_is_using_pointer` and open popups) and ignores the keyboard while an
  egui text field or drag value has focus (`ui_kbd`). Both are written at the
  end of each egui pass and read by the Update systems one frame later.
  Ctrl+S, Ctrl+O and Tab are consumed inside the egui pass (Tab would
  otherwise move egui's keyboard focus).

## Controls

- Camera: WASD, Space/C, right mouse to look, Shift for speed. It runs on
  real time because the virtual clock is paused.
- Selecting and moving: left click selects (ray against per-kind spheres).
  Dragging moves the selection on the walkmesh (the ray is marched against
  `WalkMesh::ground`); Shift-drag changes Z only. Handles: see above.
- Rotating: Q/E (15 degrees), R + wheel, or the ring knob.
- Del deletes, Ctrl+D duplicates (+1 m X), Ctrl+Z/Ctrl+Y undo/redo,
  Ctrl+S saves.
- Tab toggles the palette, Ctrl+O the Load Map modal, F10 leaves the editor.
  F1 (game fly toggle) is not blocked inside it; avoid it there.
- Markers: waypoints (flag + facing), trigger/encounter outlines, sound
  spheres, store cubes, camera spheres, and a yellow box on the selection.
- Main menu: the Editor button (`gui/menus.rs` `add_editor_button`) is the
  layout's hidden BTN_MOREGAMES control, renamed BTN_EDITOR and moved to the
  Exit row; Exit steps down one row. Its MOVETO data is swapped with the
  places (`GuiDef::swap_places`), so the data keeps Options, Editor, Exit
  order. No focus code reads MOVETO yet, so keyboard navigation does not use
  it. If the layout lacks either button, BTN_MOREGAMES is hidden instead.
  Clicking it enters `editor::START_MODULE` (101per)
  through `menu::start_editor`, which requests the editor's Load Map list
  only once the module is entered, as `--edit` without a module does.
- Not done: a rotation handle for triggers and encounters, polygon point
  editing after placement.

## Entering: black screen bug (fixed)

`--edit 101per` showed only the editor panels over black. Causes:

- Fade: 101per's on-enter script fades the screen to black for the opening
  cutscene (`Fade.target = 1`). `--edit` enters on the frame the area loads
  and pauses the world, so nothing ever faded it back in; the hud's
  fullscreen FadeOverlay (z 100, under the editor panels at z 200) stayed
  opaque. The editor now forces `Fade` clear every frame while active
  (`clear_fade`) and restores the game's fade on leaving.
- Camera: on the same frame the follow camera had never run, so the camera
  was still at the spawn default (0,0,5) instead of the party. `--edit`
  (and F10 when the camera is more than 100 m from the party) now frames the
  entry point from 14 m up and 9 m back, looking down at it.
- HUD and dialog: the game HUD (portraits, bars, minimap, action bar) and the
  conversation letterbox bars (modules that start in a conversation, 262tel)
  stayed on screen over the area. The editor hides the `GameHud` root while
  active (restored on leave) and `dialog_ui::show_dialog` shows nothing while
  `PauseState::editor` is set.

Check: `kotor2 --edit MODULE --screenshot PATH --frames 120` must show the
area with the editor panels; log lines `editor: entering via --edit|F10,
party at ...` give the framing target. `K2_EDIT_AT=FRAMES` opens the editor
that many frames after start through the F10 path (use with `--module`).

## Verification

- `kotor2 --edit-selftest 101per`: the unchanged document equals the
  original GIT tree, and its written `unchanged.mod` re-parses identically
  (all 9 list counts match). It writes `edited.mod` too and reads both back
  through `Resources` with the capsule mounted as a mod (the game's lookup
  path). Output in /tmp/k2research/edit_selftest/ (`K2_EDIT_SELFTEST_DIR`
  overrides). It exits before any window opens.
- `cargo test --release -p k2-res`: `write_mod` round trip (names, sizes,
  an empty payload), long-name rejection, mod-entry order (capsule before
  loose folder before override/), and a broken capsule skipped while the
  rest mount.
- `K2_MOD_DIR=<unchanged> k2world list 101per` diffs to 0 lines against the
  stock list.
- `cargo test --release -p kotor2`: Load Map names for 001ebo, 101per,
  904mal, 907mal and 853nih (`doc::map_name`), and the main menu's keyboard
  order after the Editor swap. Each prints SKIP when no install is found.
- Scripted edits: move a placeable +2 m, delete one, add a placeable and a
  creature, and fork a creature's conversation to "edtest"; undo and redo
  round-trip. k2world shows exactly those changes, and the game run with
  `--mod-dir` shows the added mining droid (screenshot pair
  /tmp/k2research/shots/modplay5_pair.png).
- `cargo test -p kotor2` covers the handle math (axis closest point,
  segment distance, bearing wrap) and the palette filter.
- Test hooks (env):
  `K2_EDIT_LOAD=MODULE` opens that module 30 frames after the editor is up
  (screenshot runs: `--edit 101per --frames 250`).
  `K2_EDIT_AT=FRAMES` / `K2_EDIT_LEAVE=FRAMES` open (F10 path, with
  `--module`) and leave the editor.
  `K2_EDIT_TEST="select=TEMPLATE;palette;search=TEXT;kind=NAME;pick;walk;grid"`
  sets UI state for screenshots.
  `K2_EDIT_SCRIPT="FRAME:cmd;..."` feeds input as Bevy messages (and
  `WindowEvent`s, which bevy_egui reads), frames counted from app start:
  `move,X,Y` (logical px), `down`/`up` (left button), `type,TEXT`,
  `key,NAME,down|up` (ctrl shift tab del enter esc f10 q e r z y s d o) and
  `dump` (logs selection, document and UI state). Example, drag the X arrow
  of the selected waypoint and undo it:
  `K2_EDIT_TEST=select=sw_mapnote001 K2_EDIT_SCRIPT="30:move,428,377;31:down;33:move,470,377;35:up;40:dump;42:key,ctrl,down;43:key,z,down;44:key,z,up;45:key,ctrl,up;50:dump"`.
- Screenshot runs hang if the test window gets no frames (workspace 4 not
  visible and vsync on). Point `K2_SETTINGS_DIR` at a scratch dir holding
  `settings.json` with `{"vsync": false}` (and `K2_SAVE_DIR`) so the run
  does not wait on frame callbacks; window logical size is then about
  857x643 at scale 1.167.

# Architecture

Target: a playable KotOR2. That means you start in a module, control the
player character in third person on the walkmesh, see creatures, placeables
and doors, open doors, talk to NPCs through dialog trees, and move between
modules.

## Crates

- `k2-res`: install discovery, resource resolution (override > module >
  texture packs > chitin). No Bevy.
- `k2-mdl`: binary MDL/MDX reader, K2-aware. No Bevy.
- `k2-data`: typed game data on top of k2-res and rakata GFF/2DA/TLK: module
  (IFO/ARE/GIT/LYT/VIS), blueprints (UTC/UTP/UTD/UTW/UTT/UTS/UTM/UTI), 2DA
  tables (appearance, placeables, genericdoors, surfacemat, heads, ...), TLK
  strings, DLG trees. No Bevy. Game coordinates.
- `k2-walk`: walkmesh queries over BWM (WOK/PWK/DWK): ground height, face and
  material lookup, walkability, movement with sliding collision, line of
  sight. No Bevy. Game coordinates.
- `k2-script`: NCS bytecode VM and the K2 engine-routine table.
- `k2-world`: the authoritative headless simulation. Objects spawned from a
  module, globals, action queues, script events and timers, the engine
  routines (ScriptHost), the dialog runtime and area transitions. Owns the
  area WalkMesh (doors toggle it). No Bevy. Game coordinates. The `kotor2`
  binary mirrors it: it reads object positions and `motion`, drains
  `WorldEvent`s, and reports player input (set_player_position, use_object,
  choose_reply). Harness: `k2world run|all|talk|talkall|list`.
- `kotor2`: the Bevy game binary. Owns all rendering, input, UI, and the
  conversion from game coordinates to Bevy coordinates.
- `k2probe`: CLI for listing, extracting and surveying resources.

Non-Bevy crates stay fast to build and are testable against the install
from `k2probe` subcommands.

## Conventions

- Game space is Z-up, right-handed, in meters. Bevy is Y-up, right-handed.
  Convert only in `kotor2`: bevy = (x, z, -y). Quaternions convert the same
  way.
- Game installs are found with `k2_res::Install::discover`, defaulting to `..`
  (the repo lives inside the game folder) or `K2_INSTALL`.
- Every crate depends on Bevy, rakata and similar through the workspace
  `[workspace.dependencies]` table, so versions are pinned once.
- The area editor's UI is egui via `bevy_egui` 0.42 (egui 0.36), the newest
  stable bevy_egui line for Bevy 0.19 (checked against the crate's
  compatibility table; 0.43 is rc and tracks Bevy main). Its context is
  attached to the camera only while editing (`docs/notes/editor.md`).
- Bevy 0.19. Under Wayland on NVIDIA, run with `WAYLAND_DISPLAY=` to force
  XWayland, which avoids a present-mode bug.
- The `kotor2` binary takes `--module NAME` and a headless-ish verification
  mode, `--screenshot PATH --frames N`, which renders N frames, saves a
  screenshot and exits. Agents verify visuals this way, not by guessing.

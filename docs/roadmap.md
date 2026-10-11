# Roadmap

Done:
- Resources, MDL (K2), data, walkmesh + pathfinding, NCS VM, world sim and
  script host, crowd collision.
- Rendering, animation/skinning, holograms, cutscene cameras, lip sync, Bink
  movies (ffmpeg).
- Player control, follow cam, area population, doors (open/bash), transitions.
- Audio: music, ambient, sound objects, VO, combat sfx, footsteps, GUI sounds.
- Dialog with facing, combat, Force powers/talents, XP and level up.
- Original .gui UI: main menu, chargen, HUD, inventory, equipment, loot,
  store, character, abilities, journal, messages, map/minimap, party
  selection, level up, workbench/lab (upgrade, create, breakdown).
- Save/load (named-field format), quick save, autosave, module memory.
- Party: leader switching, followers, party AI powers.
- In-engine editor (docs/notes/editor.md) with mod folders (--mod-dir), now on egui with Load Map.
- Hostile AI, door timing, plasma torch, weapon sets, shadows, graphics options,
  control channel and restart workflow (see docs/handover.md).
- MCP walk-then-act verbs with door routing, walk animation, cutscene hold,
  door message box (2026-10-09: test-verified, not reviewed or staged).

Open:
- Plasma torch sound, map arrow check. Full open list: docs/handover.md.
- Review and stage the 2026-10-09 work; rerun talkall, the doorway probe and
  ci.sh.
- Security desk camera-view pictures.
- Rev-walk review changes: use_item/use_power at range, skill verb defects,
  leader_clearance vs R_MAX, PLAYER_RADIUS, diagnose_* tests in control.rs.
- Quick bar entries for creatures (talk, Use) and a Security skill icon check.
- Editor: pack mods as .mod capsules; main-menu entry; area names in the map list.
- Minigames (swoop, turret, pazaak) are built; they need hand testing (docs/notes/minigames.md).
- Stealth, mines and traps (set/disarm), Bash on placeables polish.
- Influence system and companion conversations (K2), workbench crafting
  formulas (approximated now), kill-XP and spell DC formulas (approximated).
- Full playthrough testing module by module (101per: a haiku playtest reached
  Atton via the east route).

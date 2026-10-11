# GFF and game data notes (KotOR2, Aspyr 1.0.2)

These are findings from reading the shipped data through `k2-data`. Use
`k2probe gff NAME.EXT [MODULE]` to dump any GFF and `k2probe survey-data` to
re-check everything.

## Survey

Run over all 82 modules: every module (IFO/ARE/GIT/LYT/VIS) loads, along with
all 1700 UTC, 825 UTD, 3223 UTP, 804 UTT, 3494 UTW, 2846 UTS and 2472 DLG.
All link indices are in range, and every room, creature, door and placeable
model plus room walkmesh resolves.

The only failures are two conversations referenced by blueprints that don't
ship anywhere: `sion.dlg` (154HAR) and `351ycs.dlg` (351NAR). The engine
presumably treats them as "no conversation".

## Module files

- `module.ifo`: the entry point is `Mod_Entry_Area` plus `Mod_Entry_X/Y/Z` and a
  facing vector `Mod_Entry_Dir_X/Y`. `Mod_Area_list` always has one area in
  vanilla data. Scripts are `Mod_On*` (101PER: Mod_OnModLoad = a_fadeonload).
- `<area>.are`: `Rooms` list (RoomName, EnvAudio, AmbientScale, ForceRating,
  DisableWeather), in LYT room order in 101PER (not checked elsewhere; match by name). Colours are DWORD
  0x00BBGGRR. Sun or Moon fields are selected by IsNight. 101PER has all
  Sun colours zero and DynAmbientColor set, so DynAmbientColor is the useful
  ambient term indoors.
- K2-only ARE fields include ChanceRain/Snow/Lightning, the
  DirtyARGB/Size/Formula/Func One..Three block (dirt overlay, which matches
  the MDL dirt fields), NoHangBack, PlayerOnly, DisableTransit and the
  StealthXP* fields.
- `<area>.git`: the instance lists are "Creature List", "Door List",
  "Placeable List", "WaypointList", "TriggerList", "SoundList", "StoreList",
  "Encounter List", "CameraList", plus a generic "List" (always empty seen
  so far) and the AreaProperties struct (ambient sound and music
  ambientmusic.2da rows).
  - Creatures, waypoints, triggers, sounds and stores use
    XPosition/YPosition/ZPosition. Doors and placeables use X/Y/Z.
  - Creatures and waypoints face along (XOrientation, YOrientation). Doors
    and placeables use `Bearing` in radians. k2-data normalizes everything to
    `bearing` = rotation about +Z of a +Y-facing model, with
    bearing = -atan2(dir_x, dir_y).
  - Doors carry instance overrides: Tag, LinkedTo, LinkedToModule,
    LinkedToFlags, TransitionDestin, plus TweakColor/UseTweakColor (also on
    placeables).
  - Triggers hold Geometry points (PointX/Y/Z) relative to the instance
    position. Area transitions (UTT Type 1) carry LinkedToModule and
    LinkedTo in the GIT instance; LinkedTo is the waypoint tag in the
    destination, e.g. 101PER -> 102PER "FROM_101PER".
  - Cameras: CameraID, Position (Vector), Orientation (Quaternion, scalar
    first), Pitch (degrees), Height, FieldOfView, MicRange.

## Blueprints

- UTC: Appearance_Type is an appearance.2da row. BodyVariation/TextureVar sit
  on the creature, but the body actually comes from the equipped armor
  (Equip_ItemList struct id = slot bitmask; 0x2 = body). The armor's
  BaseItem goes to baseitems.2da `bodyvar` (a letter, e.g. Jedi_Robe = I,
  Basic_Clothing = B), and the UTI TextureVar picks the texture number.
- appearance.2da `modeltype`: B = body+head (model<letter>, tex<letter> +
  NN, normalhead to heads.2da `head`); F/S/L = a single model from `race`,
  with optional `racetex`.
  - Party NPCs like Kreia (455) are F with modela..n all the same model.
  - Some rows name textures that don't ship: racetex n_gand and n_talia,
    tex+variation n_smuggler04 + 06, n_commkidf04, n_commkidm03. k2-data
    falls back through tex+NN, tex01, the bare tex, then the model's own
    textures.
- UTP: Appearance is a placeables.2da row (`modelname`). UTD: GenericType is
  a genericdoors.2da row (`modelname`). UTD Appearance is 0 in K2 data.
- UTW data is mostly duplicated inline in the GIT waypoint entries.

## DLG (K2 additions over K1)

- Nodes have Script and Script2, each with five int parameters
  (ActionParam1..5 and ActionParam1b..5b) and a string parameter
  (ActionParamStrA/B). In K2, scripts read these via GetScriptParameter
  routines.
- Links (StartingList, RepliesList, EntriesList) have Active and Active2,
  each with Param1..5 / Param1b..5b, ParamStrA/B, a negation flag (Not,
  Not2) and `Logic` to combine them. k2-data treats Logic 0 as AND and 1 as
  OR. That comes from community docs and hasn't been checked against the
  engine.
- Other K2 node fields: Emotion (emotion.2da row), FacialAnim,
  NodeUnskippable, PostProcNode, AlienRaceNode (alienvo.2da row), RecordVO,
  RecordNoVOOverri, VOTextChanged, NodeID, CamVidEffect (-1 = none),
  FadeType. The alien voice rule is in audio.md.
- Delay 0xFFFFFFFF means the default timing.
- Top-level K2 fields: AlienRaceOwner (alienvo.2da row), PostProcOwner,
  RecordNoVO, NextNodeID,
  DeletedVOFiles, AmbientTrack, plus StuntList (Participant, StuntModel)
  used for cutscene stunt models.
- K1 CameraID/CameraAnimation fields are read when present (optional).

## 2DA

- rakata decodes "****" as an empty cell. `Table::str` treats both as None.
- appearance.2da has 671 rows; walkdist/rundist are movement speeds (m/s).
- `k2probe 2da NAME [COLUMN...]` prints a 2DA as tab-separated rows (row
  label first; "****" for empty cells).

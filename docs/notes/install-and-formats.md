# Install layout and format notes (Aspyr Linux build 1.0.2, changelist 133780)

## Layout

- Native 32-bit i386 ELF `KOTOR2`, stripped. Data under `steamassets/`.
- All shipped file names are lowercase. Mods/Workshop content may not be, so
  lookups are case-insensitive.
- `chitin.key` is KEY V1.0 and references 11 BIFFV1 archives in `data/`,
  including an Aspyr-added `legacy.bif`. 18439 resources.
- Modules use the K2 split: `name.rim` (area), `name_s.rim` (blueprints,
  scripts) and `name_dlg.erf` (dialogs). A `name.mod` replaces all three when
  present. 82 modules.
- `texturepacks/swpc_tex_{tpa,tpb,tpc,gui}.erf` are ERF V1.0 holding only
  TPC (type 3007). tpa is high quality.
- `lips/*_loc.mod`: lip sync per module.
- `streamvoice/`, `streammusic/`, `streamsounds/`: `.wav` files are mostly MP3.
  Some have a fake RIFF header with MP3 frames at 0x3A, others are bare MP3.
  Sniff the content, do not trust the extension.
- `movies/*.bik`: Bink 1 video (ffmpeg can decode it).

## Lookup order

override > loaded module capsules > texture packs (TPC only) > chitin.

## Parser survey (rakata rev 6377ce37, K1-targeted)

Run over chitin, texture packs and all 82 modules:

- 2DA 424/424, BWM 1864/1864, GFF 9612/9612, LYT 86/86: all clean.
- VIS 80/81. `710kor.vis` under-delivers child lines: `710kori 8` lists 7,
  `710korn 12` lists 11. Shipped data bug the engine tolerates, so our
  reader must too (take the lines that are there, not the count).
- TPC 15565/15571. `script_c.tpc` and `script_h.tpc` (4x4 DXT, header
  declares 3 mips) are 152 bytes: only 24 payload bytes follow the 128-byte
  header, fewer than the header implies. Lenient read needed.
- MDL: rakata's reader has no K2 support. K2 trimesh headers carry 8 extra
  bytes after `render`: dirt_enabled u8, pad, dirt_texture u16,
  dirt_coord_space u16, hide_in_holograms u8, pad. Every later mesh field
  shifts by +8 and subclass extras (skin, dangly, aabb, ...) start 8 bytes
  later. Read with K1 offsets, 898 models fail outright and the rest parse
  silently wrong. K2 is detected from geometry header fn_ptr1 (PC 0x416310
  per kotorblender, Xbox 0x4165B0). This block is the only K2-only difference
  in the binary MDL; node headers, skin, dangly and controllers match K1.
  Fixed in `crates/k2-mdl`: 3237/3237 models parse and pass sanity checks
  (vertex counts match, face indices in range, finite coordinates).

## Scripting

K2 has its own engine routine table (877 routines vs 772 in K1). The
authoritative signature list is the game's own `nwscript.nss` in scripts.bif.

//! KotOR / KotOR2 binary MDL/MDX reader.
//!
//! Vendored from rakata (https://codeberg.org/Synchro/rakata), crate
//! rakata-formats, module `mdl`, rev 6377ce37c278953c5613940253b579ee8dbee4e2,
//! GPL-3.0-or-later. Changes from upstream:
//! - binary reader only: the ASCII reader/writer and binary writer are dropped
//! - KotOR2 support: game detected from the geometry header fn_ptr1; K2
//!   TriMesh headers carry 8 extra bytes after `render` (dirt and hologram
//!   fields, see [`MdlMeshTsl`]), shifting every later field and all mesh
//!   subclass extras by +8
//! - tracing macros removed; binary helpers come from rakata-formats
//!
//! Upstream module documentation follows.
//!
//!
//! MDL is the 3D model format used by the Odyssey engine (KotOR/KotOR2).
//! Each model consists of two files: an MDL file containing the node tree,
//! controllers, and metadata, and a companion MDX file containing
//! interleaved per-vertex attribute data.
//!
//! The binary variant uses a 12-byte wrapper followed by a memory-mapped
//! content blob. All internal offsets are content-relative (byte 0 = wrapper
//! end = on-disk byte 12).
//!
//! ## Navigating the blob
//!
//! Past the two headers almost nothing sits at a fixed address. Every block
//! is reached by reading a pointer out of an earlier one, and those pointers
//! are content-relative, so a reader adds the 12-byte wrapper to each before
//! seeking. Vanilla lays the blocks out as name table, then animation
//! headers, then the node tree, but that ordering is a convention rather
//! than something the format enforces; the pointers are what locate a block.
//!
//! - The **geometry header** starts the content, and the **model header**
//!   follows it immediately. These two are the only fixed addresses.
//! - The **node tree** hangs off a root pointer in the geometry header, and
//!   is walked by recursing each node's own child array. Every node begins
//!   with the same 80-byte header; the type flags in it select what data
//!   follows.
//! - **Animation headers** come from a counted array in the model header,
//!   and each one roots its own node tree.
//! - **Node names** are indirect twice over: the model header points at an
//!   array of offsets, and each entry points at a NUL-terminated string.
//! - **Vertex data is not in this file.** It lives in the companion MDX,
//!   addressed per mesh by an offset relative to that file rather than this
//!   one. Confusing the two is the bug this reader is most prone to.
//!
//! Byte-level field maps are deliberately not repeated here, because two
//! copies of an offset table drift and this one already had: the header
//! disagreed with the manual about the width of `supermodel_name` until a
//! corpus check settled it. They live in
//! `docs/src/formats/models/mdl.md`, with the engine-side load pipeline in
//! `docs/src/internals/mdl_deep_dive.md`.
//!
//! ## Two traps worth knowing before writing a reader
//!
//! **The file and the loaded struct are different maps.** The engine copies
//! the blob into memory and rewrites relative offsets in place, so several
//! fields hold one thing on disk and another once resident. Content `+0x4C`
//! is a plain `2` in the file and `GetType() | 0x80` after load. The page
//! documents the file; the deep dive documents the loaded form.
//!
//! **`node_count` is not the number of nodes in the file.** It counts the
//! resolved supermodel chain, so validating it against the nodes actually
//! reachable rejects hundreds of correct retail models. For a model with no
//! supermodel the two agree exactly.

macro_rules! trace_debug {
    ($($arg:tt)*) => {};
}
macro_rules! trace_warn {
    ($($arg:tt)*) => {};
}
pub(crate) use trace_debug;
pub(crate) use trace_warn;

/// Controller types and keyframe data.
pub mod controllers;
/// Quaternion / axis-angle helpers.
pub mod orientation;
/// Binary MDL reader.
pub mod reader;
/// Node data types.
pub mod types;

pub use controllers::{MdlController, MdlControllerType, MdlKey};
pub use reader::{read_mdl, read_mdl_from_bytes};
pub use types::{
    AabbNode, MdlAabb, MdlAnimMesh, MdlCamera, MdlDangly, MdlEmitter, MdlFace, MdlLight, MdlMesh,
    MdlMeshTsl, MdlNodeData, MdlReference, MdlSaber, MdlSkin,
};

use rakata_formats::binary::DecodeBinary;

/// Animation header size in the binary format (136 bytes = 0x88).
///
/// Mirrors the geometry header structure: fn_ptrs (8) + name (32) +
/// root_node_ptr (4) + node_count (4) + runtime_arrays (24) + ref_count (4) +
/// model_type (4) + length (4) + transition (4) + anim_root (32) +
/// event_arr (12) + padding (4).
///
/// Verified against kotorblender's `peek_animations` (136 bytes per header)
/// and binary reader at `load_animation`.
pub(crate) const ANIMATION_HEADER_SIZE: usize = 0x88;

/// Event size in the binary format (36 bytes = 0x24).
///
/// Layout: time (f32, 4 bytes) + name (char[32], 32 bytes).
pub(crate) const ANIMATION_EVENT_SIZE: usize = 0x24;

/// Offsets within an animation header (relative to animation header start).
pub(crate) mod anim_header_offsets {
    /// Function pointer 1 (u32).
    pub const FN_PTR1: usize = 0x00;
    /// Function pointer 2 (u32).
    pub const FN_PTR2: usize = 0x04;
    /// Animation name (32-byte null-terminated string).
    pub const NAME: usize = 0x08;
    /// Name field size.
    pub const NAME_SIZE: usize = 32;
    /// Content-relative offset to animation root node.
    pub const ROOT_NODE_PTR: usize = 0x28;
    /// Total number of animation nodes.
    #[allow(dead_code)]
    pub const NODE_COUNT: usize = 0x2C;
    /// Runtime array 1 (12 bytes, zeros on disk).
    #[allow(dead_code)]
    pub const RUNTIME_ARR1: usize = 0x30;
    /// Runtime array 2 (12 bytes, zeros on disk).
    #[allow(dead_code)]
    pub const RUNTIME_ARR2: usize = 0x3C;
    /// Reference count (zero on disk).
    #[allow(dead_code)]
    pub const REF_COUNT: usize = 0x48;
    /// Model type byte (always 5 for animations).
    #[allow(dead_code)]
    pub const MODEL_TYPE: usize = 0x4C;
    /// Animation duration in seconds (f32).
    pub const LENGTH: usize = 0x50;
    /// Transition time in seconds (f32).
    pub const TRANSITION: usize = 0x54;
    /// Animation root node name (32-byte null-terminated string).
    pub const ANIM_ROOT: usize = 0x58;
    /// Animation root name field size.
    pub const ANIM_ROOT_SIZE: usize = 32;
    /// Event array pointer (content-relative, u32).
    pub const EVENT_ARR_PTR: usize = 0x78;
    /// Event count (u32).
    pub const EVENT_ARR_COUNT: usize = 0x7C;
    /// Event array allocated count (u32, mirrors count on disk).
    #[allow(dead_code)]
    pub const EVENT_ARR_ALLOC: usize = 0x80;
    /// Padding (4 bytes at +0x84).
    #[allow(dead_code)]
    pub const PADDING_84: usize = 0x84;
}

/// K1 PC animation function pointer 1 (`0x00413370`).
#[allow(dead_code)]
pub(crate) const ANIM_FN_PTR_1_K1_PC: u32 = 4_273_392;
/// K1 PC animation function pointer 2 (`0x0043E1E0`).
#[allow(dead_code)]
pub(crate) const ANIM_FN_PTR_2_K1_PC: u32 = 4_451_552;

/// MDL file wrapper size (12 bytes: zero_marker + mdl_size + mdx_size).
pub(crate) const MDL_WRAPPER_SIZE: u64 = 12;

/// Offsets within the Model Header (relative to wrapper end).
///
/// Verified against `InputBinary::Reset` (`0x004a1030`) and `Model::Model`
/// (`0x0044aa70`) in `swkotor.exe`, cross-validated via hex dump of
/// `c_dewback.mdl` (Character=4), `dor_lhr01.mdl` (Door=8),
/// and `m01aa_01a.mdl` (Other=0).
///
/// See `docs/src/internals/mdl_deep_dive.md` "The core idea: load-and-fixup".
#[allow(dead_code)] // full layout map; not every field is read
pub(crate) mod header_offsets {
    // --- Geometry header (80 bytes, +0x00..+0x4F) ---

    /// Function pointer 1 (u32). Used by kotorblender for K1/K2/Xbox detection.
    /// Runtime vtable pointer leaked from the BioWare toolset.
    pub const FN_PTR1: usize = 0x00;
    /// Function pointer 2 (u32). Same provenance as fn_ptr1.
    pub const FN_PTR2: usize = 0x04;
    /// Model name (32-byte null-terminated string at +0x08).
    pub const MODEL_NAME: usize = 0x08;
    /// Size of the model name field.
    pub const MODEL_NAME_SIZE: usize = 32;
    /// Offset to the root node structure.
    pub const ROOT_NODE_PTR: usize = 0x28;
    /// Total number of nodes in the model.
    pub const NODE_COUNT: usize = 0x2C;
    /// Runtime array 1 (12 bytes: ptr/count/alloc). Zeroed on disk.
    #[allow(dead_code)]
    pub const RUNTIME_ARR1: usize = 0x30;
    /// Runtime array 2 (12 bytes: ptr/count/alloc). Zeroed on disk.
    #[allow(dead_code)]
    pub const RUNTIME_ARR2: usize = 0x3C;
    /// Reference count (u32). Runtime-only, zero on disk.
    #[allow(dead_code)]
    pub const REF_COUNT: usize = 0x48;
    /// Model type (u8): 0=geometry, 5=animation. Always 2 for geometry in KotOR.
    pub const MODEL_TYPE: usize = 0x4C;

    // --- Model header (116 bytes, +0x50..+0xC3) ---

    /// Model classification byte (0=Other, 1=Effect, 2=Tile, 4=Character, 8=Door).
    ///
    /// Verified via `Model::Model` constructor (`0x0044aa70`): field at +0x50,
    /// default 0. Cross-validated against 3 vanilla K1 models.
    pub const CLASSIFICATION: usize = 0x50;
    /// Subclassification byte (+0x51). Non-zero in ~196 vanilla K1 models.
    pub const SUBCLASSIFICATION: usize = 0x51;
    /// Unknown byte (+0x52). Always 0 in vanilla.
    #[allow(dead_code)]
    pub const UNKNOWN_52: usize = 0x52;
    /// Affected-by-fog flag (+0x53). 0 or 1.
    pub const AFFECTED_BY_FOG: usize = 0x53;
    /// Number of child models (+0x54). Always 0 in vanilla K1.
    #[allow(dead_code)]
    pub const NUM_CHILD_MODELS: usize = 0x54;
    /// Animation offsets CExoArrayList (ptr/count/alloc, 12 bytes at +0x58).
    pub const ANIMATION_ARR_PTR: usize = 0x58;
    /// Animation count.
    pub const ANIMATION_ARR_COUNT: usize = 0x5C;
    /// Supermodel reference (u32 at +0x64). Always 0 in vanilla K1.
    #[allow(dead_code)]
    pub const SUPERMODEL_REF: usize = 0x64;
    /// Model bounding box minimum (3 x f32, +0x68..+0x73).
    pub const BOUNDING_BOX_MIN: usize = 0x68;
    /// Model bounding box maximum (3 x f32, +0x74..+0x7F).
    #[allow(dead_code)]
    pub const BOUNDING_BOX_MAX: usize = 0x74;
    /// Model bounding sphere radius (f32, +0x80).
    pub const RADIUS: usize = 0x80;
    /// Animation scale factor (f32, default 1.0).
    ///
    /// Verified via `Model::Model` constructor: field at +0x84, initialized to 1.0.
    pub const ANIMATION_SCALE: usize = 0x84;
    /// Supermodel name (null-terminated string, up to 32 chars within 32-byte field).
    ///
    /// Verified via `InputBinary::Reset`: `FindModel((char*)(buf+0x88))`.
    /// Constructor initializes to `'\0'` (empty string = no supermodel).
    pub const SUPERMODEL_NAME: usize = 0x88;
    /// Size of the supermodel name field in the binary header.
    pub const SUPERMODEL_NAME_SIZE: usize = 32;
    /// Animation root node offset (+0xA8). Content-relative pointer.
    pub const OFF_ANIM_ROOT: usize = 0xA8;
    /// MDX total size (+0xB0). Writer derives from MDX buffer.
    pub const MDX_SIZE: usize = 0xB0;
    /// MDX offset (+0xB4). Always 0 in vanilla K1.
    #[allow(dead_code)]
    pub const MDX_OFFSET: usize = 0xB4;
    /// Offset to the array of name string pointers.
    pub const NAME_OFFSETS_PTR: usize = 0xB8;
    /// Number of names in the name table.
    pub const NAME_COUNT: usize = 0xBC;
}

/// Size of the base node header in the binary format (bytes 0x00-0x4F).
///
/// The binary format uses 3-field arrays (ptr, count_used, count_allocated)
/// for children, controller keys, and controller data - verified empirically
/// against `c_dewback.mdl` extracted from vanilla K1 via `vanilla-inspector`.
pub(crate) const NODE_HEADER_SIZE: usize = 0x50;

/// Extra header size for Light nodes (92 bytes).
///
/// Verified via Ghidra struct `MdlNodeLight` (172 total - 80 base = 92).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const LIGHT_EXTRA_SIZE: usize = 0x5C;

/// Offsets within a Light extra header (relative to light extra start,
/// i.e. byte 0x50 from the node base, immediately after the base header).
///
/// Verified via `InputBinary::ResetLight` (`0x004a05e0`) and
/// `MdlNodeLight::InternalParseField` (`0x00469150`).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) mod light_offsets {
    /// Flare radius (f32). Extra +0x00.
    pub const FLARE_RADIUS: usize = 0x00;
    /// Texture SafePointers CExoArrayList (12 bytes, runtime-only). Extra +0x04.
    /// Zeroed on disk - populated at runtime by `AurTextureGetReference`.
    pub const TEXTURE_SAFE_PTRS_PTR: usize = 0x04;
    /// Flare sizes CExoArrayList pointer (u32, relocated). Extra +0x10.
    pub const FLARE_SIZES_PTR: usize = 0x10;
    /// Flare sizes count (u32). Extra +0x14.
    pub const FLARE_SIZES_COUNT: usize = 0x14;
    // 0x18: CExoArrayList allocated count (ignored)
    /// Flare positions CExoArrayList pointer (u32, relocated). Extra +0x1C.
    pub const FLARE_POSITIONS_PTR: usize = 0x1C;
    /// Flare positions count (u32). Extra +0x20.
    pub const FLARE_POSITIONS_COUNT: usize = 0x20;
    // 0x24: CExoArrayList allocated count (ignored)
    /// Flare color shifts CExoArrayList pointer (u32, relocated). Extra +0x28.
    pub const FLARE_COLOR_SHIFTS_PTR: usize = 0x28;
    /// Flare color shifts count (u32). Extra +0x2C.
    pub const FLARE_COLOR_SHIFTS_COUNT: usize = 0x2C;
    // 0x30: CExoArrayList allocated count (ignored)
    /// Flare texture names CExoArrayList pointer (u32, relocated). Extra +0x34.
    ///
    /// Points to an array of u32 string offsets, each of which is also relocated.
    /// The strings are null-terminated.
    pub const FLARE_TEX_NAMES_PTR: usize = 0x34;
    /// Flare texture names count (u32). Extra +0x38.
    pub const FLARE_TEX_NAMES_COUNT: usize = 0x38;
    // 0x3C: CExoArrayList allocated count (ignored)
    /// Light priority (i32, default 5). Extra +0x40.
    pub const PRIORITY: usize = 0x40;
    /// Dynamic type count (i32, default 1). Extra +0x44.
    pub const NUM_DYNAMIC_TYPES: usize = 0x44;
    /// Affects dynamic objects (i32, default 1). Extra +0x48.
    pub const AFFECTDYNAMIC: usize = 0x48;
    /// Casts shadow (i32, default 1). Extra +0x4C.
    pub const SHADOW: usize = 0x4C;
    /// Ambient-only light (i32, default 0). Extra +0x50.
    pub const AMBIENTONLY: usize = 0x50;
    /// Generate flare effect (i32, default 0). Extra +0x54.
    pub const GENERATEFLARE: usize = 0x54;
    /// Fading light (i32, default 1). Extra +0x58.
    pub const FADING_LIGHT: usize = 0x58;
}

/// Extra header size for Reference nodes (36 bytes = char[32] + i32).
///
/// Verified via Ghidra struct `MdlNodeReference` (116 total - 80 base = 36).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const REFERENCE_EXTRA_SIZE: usize = 0x24;

/// Extra header size for Emitter nodes (224 bytes).
///
/// All inline fixed-size data - no pointer relocation needed.
/// Verified via Ghidra struct `MdlNodeEmitter` (304 total - 80 base = 224).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const EMITTER_EXTRA_SIZE: usize = 0xE0;

/// Offsets within a Node Header (0x50 = 80 bytes).
///
/// Array fields use 3 x u32 (ptr, count_used, count_allocated) - the allocated
/// count mirrors used count on disk and is ignored during reading.
///
/// Empirically verified against `c_dewback.mdl` (102 nodes, vanilla K1).
/// See `docs/src/internals/mdl_deep_dive.md` "Type dispatch".
pub(crate) mod node_offsets {
    /// Node type flags (u16).
    pub const FLAGS: usize = 0x00;
    /// Index into the model's name table (u16).
    ///
    /// This was called `node_id` and documented as a redundant copy of the
    /// `u16` at `+0x02`, on a four-file sample where the two happened to
    /// agree. Across every node in every model indexed by a retail
    /// `chitin.key` they disagree in roughly one node in six, and this one
    /// resolves through the name-offsets array to a terminated string every
    /// time. It is the name index; `+0x02` is the node number.
    pub const NAME_INDEX: usize = 0x04;
    /// Local position X coordinate (f32).
    pub const POS_X: usize = 0x10;
    /// Orientation quaternion W component (f32). Layout: w, x, y, z.
    ///
    /// Verified via `Quaternion` struct in Ghidra: field order is {w, x, y, z}.
    pub const ORIENTATION_W: usize = 0x1C;
    /// Offset to the array of child node pointers.
    pub const CHILD_ARRAY_PTR: usize = 0x2C;
    /// Number of child nodes (used count).
    pub const CHILD_COUNT: usize = 0x30;
    // 0x34: child array allocated count (ignored)
    /// Offset to the array of controller key headers.
    pub const CONTROLLER_KEY_PTR: usize = 0x38;
    /// Number of controller keys (used count).
    pub const CONTROLLER_KEY_COUNT: usize = 0x3C;
    // 0x40: controller key array allocated count (ignored)
    /// Offset to the array of controller data (floats).
    pub const CONTROLLER_DATA_PTR: usize = 0x44;
    /// Number of controller data elements (floats, used count).
    pub const CONTROLLER_DATA_COUNT: usize = 0x48;
    // 0x4C: controller data array allocated count (ignored)
}

/// Extra header size for TriMesh nodes (332 bytes = 0x14C).
///
/// MdlNodeTriMesh is 412 bytes total; the base MdlNode is 80 bytes.
/// Verified via Ghidra struct `MdlNodeTriMesh` (412 bytes total).
/// See `docs/src/internals/mdl_deep_dive.md` "TriMesh".
pub(crate) const MESH_EXTRA_SIZE: usize = 0x14C;

/// Bytes KotOR2 inserts into the TriMesh header after `render` (+0x139).
pub(crate) const TSL_MESH_EXTRA: usize = 8;

/// Model geometry header fn_ptr1 values, used to tell the games apart.
pub mod game_fn_ptr {
    /// KotOR PC.
    pub const K1_PC: u32 = 4_273_776;
    /// KotOR2 PC.
    pub const K2_PC: u32 = 4_285_200;
    /// KotOR Xbox.
    pub const K1_XBOX: u32 = 4_254_992;
    /// KotOR2 Xbox.
    pub const K2_XBOX: u32 = 4_285_872;
}

/// Extra header size for Skin nodes beyond TriMesh (100 bytes = 0x64).
///
/// MdlNodeSkin is 512 bytes total (412 TriMesh + 100 extra).
/// Verified via Ghidra struct `MdlNodeSkin` and `InputBinary::ResetSkin`
/// (`0x004a01b0`). See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const SKIN_EXTRA_SIZE: usize = 0x64;

/// Extra header size for AnimMesh nodes beyond TriMesh (56 bytes = 0x38).
///
/// MdlNodeAnimMesh is 468 bytes total (412 TriMesh + 56 extra).
/// Verified via Ghidra struct `MdlNodeAnimMesh` and `InputBinary::ResetAnim`
/// (`0x004a0060`). See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const ANIM_MESH_EXTRA_SIZE: usize = 0x38;

/// Extra header size for AABB nodes beyond TriMesh (4 bytes).
///
/// MdlNodeAABB is 416 bytes total (412 TriMesh + 4 extra).
/// The 4-byte extra is a root pointer to the AABB binary search tree,
/// which is stored inline in the MDL content blob as a flattened binary tree.
///
/// Verified via `ResetMdlNode` inline processing and `ResetAABBTree`
/// (`0x004a0260`). See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const AABB_EXTRA_SIZE: usize = 0x04;

/// Extra header size for Saber nodes beyond TriMesh (20 bytes = 0x14).
///
/// MdlNodeLightsaber is 432 bytes total (412 TriMesh + 20 extra).
/// Contains 3 relocated data pointers and 2 runtime GL pool IDs.
///
/// Verified via `InputBinary::ResetLightsaber` (`0x004a0460`) and
/// `ParseNode` allocation (`operator_new(0x1B0)`).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const SABER_EXTRA_SIZE: usize = 0x14;

/// Extra header size for DanglyMesh nodes beyond TriMesh (28 bytes = 0x1C).
///
/// MdlNodeDanglyMesh is 440 bytes total (412 TriMesh + 28 extra).
/// Verified via Ghidra struct `MdlNodeDanglyMesh` and `InputBinary::ResetDangly`
/// (`0x004a0100`). See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) const DANGLY_EXTRA_SIZE: usize = 0x1C;

/// Size of a single MaxFace entry in the face array (32 bytes).
///
/// Layout: plane_normal(3xf32) + plane_distance(f32) + surface_id(u32)
/// + adjacent(3xu16) + vertex_indices(3xu16).
///
/// Verified via Ghidra struct `MaxFace` (32 bytes) and the `0x1A` vertex-index
/// offset constant in `InternalGenVertices` functions.
/// See `docs/src/internals/mdl_deep_dive.md` "TriMesh".
pub(crate) const MAX_FACE_SIZE: usize = 32;

/// Offsets within a TriMesh extra header (relative to mesh extra start, i.e.
/// `MdlNodeTriMesh` absolute offset 0x50).
///
/// Verified against `InputBinary::ResetTriMeshParts` (`0x004a0c00`)
/// and Ghidra struct `MdlNodeTriMesh` (412 bytes).
/// See `docs/src/internals/mdl_deep_dive.md` "TriMesh".
#[allow(dead_code)] // full layout map; not every field is read
pub(crate) mod mesh_offsets {
    /// `gen_vertices` function pointer stub (u32). Extra +0x00.
    ///
    /// Stale code address from BioWare's build toolset. Overwritten at runtime
    /// by the engine constructor. Preserved for lossless roundtrip and as a
    /// toolset version fingerprint.
    pub const FN_PTR_GEN_VERTICES: usize = 0x00;
    /// `remove_temporary_array` function pointer stub (u32). Extra +0x04.
    pub const FN_PTR_REMOVE_TEMP_ARRAY: usize = 0x04;

    /// Face CExoArrayList data pointer (u32, relocated). Extra +0x08.
    pub const FACE_ARRAY_OFFSET: usize = 0x08;
    /// Number of faces (u32, CExoArrayList.size). Extra +0x0C.
    pub const FACE_COUNT: usize = 0x0C;

    /// Bounding box minimum (3xf32). Extra +0x14.
    pub const BOUNDING_MIN: usize = 0x14;
    /// Bounding box maximum (3xf32). Extra +0x20.
    pub const BOUNDING_MAX: usize = 0x20;
    /// Bounding sphere radius (f32). Extra +0x2C.
    pub const BSPHERE_RADIUS: usize = 0x2C;
    /// Bounding sphere center (3xf32). Extra +0x30.
    pub const BSPHERE_CENTER: usize = 0x30;
    /// RGB diffuse color (3xf32). Extra +0x3C.
    pub const DIFFUSE_COLOR: usize = 0x3C;
    /// RGB ambient color (3xf32). Extra +0x48.
    pub const AMBIENT_COLOR: usize = 0x48;
    /// Transparency hint (i32, 0=opaque, 1=transparent). Extra +0x54.
    pub const TRANSPARENCY_HINT: usize = 0x54;
    /// Primary texture name (char[32], null-terminated). Extra +0x58.
    pub const TEXTURE_0: usize = 0x58;
    /// Size of each texture name field in bytes.
    pub const TEXTURE_NAME_SIZE: usize = 32;
    /// Secondary/lightmap texture name (char[32], null-terminated). Extra +0x78.
    pub const TEXTURE_1: usize = 0x78;

    /// `vertex_indices` CExoArrayList pointer (u32, relocated). Extra +0x98.
    /// Dead field in KotOR - always zeros. Reader/writer skip it.
    #[allow(dead_code)]
    pub const VERTEX_INDICES_ARRAY_PTR: usize = 0x98;
    /// `vertex_indices` CExoArrayList count (u32). Extra +0x9C.
    #[allow(dead_code)]
    pub const VERTEX_INDICES_ARRAY_COUNT: usize = 0x9C;
    /// `vertex_indices` CExoArrayList allocated count (u32). Extra +0xA0.
    #[allow(dead_code)]
    pub const VERTEX_INDICES_ARRAY_ALLOC: usize = 0xA0;

    /// `left_over_faces` CExoArrayList pointer (u32, relocated). Extra +0xA4.
    pub const LEFT_OVER_FACES_ARRAY_PTR: usize = 0xA4;
    /// `left_over_faces` CExoArrayList count (u32). Extra +0xA8.
    #[allow(dead_code)]
    pub const LEFT_OVER_FACES_ARRAY_COUNT: usize = 0xA8;
    /// `left_over_faces` CExoArrayList allocated count (u32). Extra +0xAC.
    #[allow(dead_code)]
    pub const LEFT_OVER_FACES_ARRAY_ALLOC: usize = 0xAC;

    /// `vertex_indices_count` CExoArrayList pointer (u32, relocated). Extra +0xB0.
    pub const VERTEX_INDICES_COUNT_ARRAY_PTR: usize = 0xB0;
    /// `vertex_indices_count` CExoArrayList count (u32). Extra +0xB4.
    pub const VERTEX_INDICES_COUNT_ARRAY_COUNT: usize = 0xB4;
    /// `vertex_indices_count` CExoArrayList allocated count (u32). Extra +0xB8.
    pub const VERTEX_INDICES_COUNT_ARRAY_ALLOC: usize = 0xB8;

    /// `mdx_offsets` CExoArrayList pointer (u32, relocated). Extra +0xBC.
    pub const MDX_OFFSETS_ARRAY_PTR: usize = 0xBC;
    /// `mdx_offsets` CExoArrayList count (u32). Extra +0xC0.
    pub const MDX_OFFSETS_ARRAY_COUNT: usize = 0xC0;
    /// `mdx_offsets` CExoArrayList allocated count (u32). Extra +0xC4.
    pub const MDX_OFFSETS_ARRAY_ALLOC: usize = 0xC4;

    /// `index_buffer_pools` CExoArrayList pointer (u32, relocated). Extra +0xC8.
    pub const INDEX_BUFFER_POOLS_ARRAY_PTR: usize = 0xC8;
    /// `index_buffer_pools` CExoArrayList count (u32). Extra +0xCC.
    pub const INDEX_BUFFER_POOLS_ARRAY_COUNT: usize = 0xCC;
    /// `index_buffer_pools` CExoArrayList allocated count (u32). Extra +0xD0.
    pub const INDEX_BUFFER_POOLS_ARRAY_ALLOC: usize = 0xD0;

    /// Shared index offset scalar (i32). Extra +0xD4.
    pub const SHARED_INDEX_OFFSET: usize = 0xD4;
    /// Shared index pool scalar (i32/pointer-sized on-disk value). Extra +0xD8.
    pub const SHARED_INDEX_POOL: usize = 0xD8;
    /// Shared index size scalar (i32). Extra +0xDC.
    pub const SHARED_INDEX_SIZE: usize = 0xDC;
    /// Indices-per-face scalar (u32). Extra +0xE0.
    pub const INDICES_PER_FACE: usize = 0xE0;

    /// UV animation enable flag (i32). Extra +0xE8.
    pub const ANIMATE_UV: usize = 0xE8;
    /// UV animation direction X (f32). Extra +0xEC.
    pub const UV_DIRECTION_X: usize = 0xEC;
    /// UV animation direction Y (f32). Extra +0xF0.
    pub const UV_DIRECTION_Y: usize = 0xF0;
    /// UV jitter amount (f32). Extra +0xF4.
    pub const UV_JITTER: usize = 0xF4;
    /// UV jitter speed (f32). Extra +0xF8.
    pub const UV_JITTER_SPEED: usize = 0xF8;

    /// Per-vertex stride in MDX data (u32).
    ///
    /// At MdlNodeTriMesh absolute +0x14C, mesh extra offset +0xFC.
    /// Verified via `ResetTriMeshParts`, which multiplies this by the vertex
    /// count to size the whole vertex block.
    pub const VERTEX_STRUCT_SIZE: usize = 0xFC;

    /// Number of vertices (u16).
    ///
    /// At MdlNodeTriMesh absolute +0x180, mesh extra offset +0x130.
    /// Verified via `ResetTriMeshParts`, which narrows the read to 16 bits.
    pub const VERTEX_COUNT: usize = 0x130;

    /// Number of UV texture channels (u16). Extra +0x132.
    pub const TEXTURE_CHANNEL_COUNT: usize = 0x132;
    /// Lightmapped flag (bool/u8). Extra +0x134.
    pub const LIGHT_MAPPED: usize = 0x134;
    /// Rotate texture flag (bool/u8). Extra +0x135.
    pub const ROTATE_TEXTURE: usize = 0x135;
    /// Background geometry flag (bool/u8). Extra +0x136.
    pub const IS_BACKGROUND_GEOMETRY: usize = 0x136;

    /// Shadow flag (bool/u8). MdlNodeTriMesh absolute +0x187, extra +0x137.
    pub const SHADOW: usize = 0x137;

    /// Beaming flag (bool/u8). Extra +0x138.
    pub const BEAMING: usize = 0x138;

    /// Render flag (bool/u8). MdlNodeTriMesh absolute +0x189, extra +0x139.
    pub const RENDER: usize = 0x139;

    /// Total surface area (f32). Extra +0x13C.
    ///
    /// Computed by `ComputeLocalSurfaceArea` during ASCII->binary post-processing.
    /// In binary MDL files, preserved as-is from the file.
    pub const TOTAL_SURFACE_AREA: usize = 0x13C;

    /// Per-mesh offset into the MDX file (u32). Extra +0x144.
    ///
    /// Stores the byte offset where this mesh's interleaved vertex data begins
    /// in the companion MDX file. The engine (and community tools like
    /// kotorblender) uses this to seek to the correct position in the MDX
    /// buffer for each mesh's vertices.
    ///
    /// Confirmed via kotorblender reader (line 375): reads this field and uses
    /// it as `mdx.seek(mdx_offset + i * stride + attr_offset)`.
    pub const MDX_DATA_OFFSET: usize = 0x144;

    /// Content-relative pointer to position-only vertex data (u32, relocated).
    ///
    /// At MdlNodeTriMesh absolute +0x198, mesh extra offset +0x148.
    /// Verified via `ResetTriMeshParts`, which relocates it against the MDL
    /// content base pointer and **not** the MDX base. The MDX base is unused
    /// and freed once the reset completes, so resolving this offset against it
    /// reads the wrong buffer.
    ///
    /// Points to `vertex_count * 12` bytes (3x f32 position data) within the
    /// MDL content blob. This is the embedded vertex position array, always
    /// present in vanilla files even when MDX data exists.
    ///
    /// See `docs/src/internals/mdl_deep_dive.md` "What this means for `mdx_data_offset`".
    pub const VERT_ARRAY_OFFSET: usize = 0x148;

    // --- MDX vertex attribute layout fields ---
    // These fields control which vertex attributes are present in the MDX
    // companion file and where each attribute sits within each vertex's stride.
    //
    // Verified via Ghidra struct `MdlNodeTriMesh` and vanilla K1 hex dumps.
    // See `docs/src/internals/mdl_deep_dive.md` "MDX vertex layout".

    /// MDX vertex attribute flags bitfield (u32). Extra +0x100.
    ///
    /// Bits: 0x01=position, 0x02=UV1, 0x04=UV2, 0x08=UV3, 0x10=UV4,
    /// 0x20=normal, 0x80=tangent_space. Vertex colors have no flag bit
    /// (presence determined by offset != -1).
    pub const MDX_VERTEX_FLAGS: usize = 0x100;
    /// Byte offset of position data within each MDX vertex (i32). Extra +0x104.
    /// Value -1 (0xFFFFFFFF) means not present.
    pub const MDX_POSITION_OFFSET: usize = 0x104;
    /// Byte offset of normal data within each MDX vertex (i32). Extra +0x108.
    pub const MDX_NORMAL_OFFSET: usize = 0x108;
    /// Byte offset of vertex color data within each MDX vertex (i32). Extra +0x10C.
    pub const MDX_COLOR_OFFSET: usize = 0x10C;
    /// Byte offset of UV1 texture coordinates within each MDX vertex (i32). Extra +0x110.
    pub const MDX_UV1_OFFSET: usize = 0x110;
    /// Byte offset of UV2 texture coordinates within each MDX vertex (i32). Extra +0x114.
    pub const MDX_UV2_OFFSET: usize = 0x114;
    /// Byte offset of UV3 texture coordinates within each MDX vertex (i32). Extra +0x118.
    pub const MDX_UV3_OFFSET: usize = 0x118;
    /// Byte offset of UV4 texture coordinates within each MDX vertex (i32). Extra +0x11C.
    pub const MDX_UV4_OFFSET: usize = 0x11C;
    /// Byte offset of tangent space data within each MDX vertex (i32). Extra +0x120.
    /// Tangent space is 3x3 floats (tangent, bitangent, cross product) = 36 bytes.
    pub const MDX_TANGENT_SPACE_OFFSET: usize = 0x120;

    /// Reserved MDX offset slot 8 (always -1 in vanilla K1). Extra +0x124.
    #[allow(dead_code)]
    pub const MDX_RESERVED_OFFSET_8: usize = 0x124;
    /// Reserved MDX offset slot 9 (always -1 in vanilla K1). Extra +0x128.
    #[allow(dead_code)]
    pub const MDX_RESERVED_OFFSET_9: usize = 0x128;
    /// Reserved MDX offset slot 10 (always -1 in vanilla K1). Extra +0x12C.
    #[allow(dead_code)]
    pub const MDX_RESERVED_OFFSET_10: usize = 0x12C;
}

/// Offsets within a Skin extra header (relative to skin extra start,
/// i.e. byte 0x19C from the node base, immediately after the TriMesh header).
///
/// Verified via `InputBinary::ResetSkin` (`0x004a01b0`) and Ghidra struct
/// `MdlNodeSkin` (512 bytes total).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) mod skin_offsets {
    /// Weights CExoArrayList header start (12 bytes). Extra +0x00.
    ///
    /// Always zeros in vanilla binary files - the engine's `SkinVertexWeight`
    /// array is only populated by the ASCII text parser.
    #[allow(dead_code)]
    pub const WEIGHTS_PTR: usize = 0x00;
    // 0x04: CExoArrayList.size (always 0)
    // 0x08: CExoArrayList.allocated (always 0)
    /// MDX per-vertex bone weights byte offset (i32). Extra +0x0C.
    ///
    /// Byte offset within each MDX vertex stride to the 4-float bone weights.
    /// Value -1 means not present.
    pub const MDX_BONE_WEIGHTS_OFFSET: usize = 0x0C;
    /// MDX per-vertex bone indices byte offset (i32). Extra +0x10.
    ///
    /// Byte offset within each MDX vertex stride to the 4-float bone indices.
    /// Value -1 means not present.
    pub const MDX_BONE_INDICES_OFFSET: usize = 0x10;
    /// Bonemap data pointer (u32, relocated if count at +0x18 != 0). Extra +0x14.
    ///
    /// Points to an array of bone mapping entries in the MDL content blob.
    pub const BONEMAP_PTR: usize = 0x14;
    /// Bonemap entry count (u32, size guard for +0x14). Extra +0x18.
    pub const BONEMAP_COUNT: usize = 0x18;
    /// Inverse bind rotation CExoArrayList pointer (u32, relocated). Extra +0x1C.
    ///
    /// Points to an array of Quaternion (4 x f32 = 16 bytes each).
    /// One entry per bone - transforms from bone space to model space.
    pub const QBONE_REF_INV_PTR: usize = 0x1C;
    /// Number of inverse bind rotations (u32, CExoArrayList.size). Extra +0x20.
    pub const QBONE_REF_INV_COUNT: usize = 0x20;
    // 0x24: CExoArrayList allocated count (ignored)
    /// Inverse bind translation CExoArrayList pointer (u32, relocated). Extra +0x28.
    ///
    /// Points to an array of Vector (3 x f32 = 12 bytes each).
    /// One entry per bone - translation from bone space to model space.
    pub const TBONE_REF_INV_PTR: usize = 0x28;
    /// Number of inverse bind translations (u32, CExoArrayList.size). Extra +0x2C.
    pub const TBONE_REF_INV_COUNT: usize = 0x2C;
    // 0x30: CExoArrayList allocated count (ignored)
    /// Bone constant indices CExoArrayList pointer (u32, relocated). Extra +0x34.
    ///
    /// Maps local bone index to global skeleton node index.
    pub const BONE_CONSTANT_INDICES_PTR: usize = 0x34;
    /// Number of bone constant indices (u32, CExoArrayList.size). Extra +0x38.
    pub const BONE_CONSTANT_INDICES_COUNT: usize = 0x38;
    // 0x3C: CExoArrayList allocated count (ignored)
    /// Bone node serial numbers array (16 x u16 = 32 bytes). Extra +0x40.
    ///
    /// Fixed-size array of 16 bone node indices. Unused slots are zero.
    pub const BONE_NODE_NUMBERS: usize = 0x40;
    // 0x40..0x5F: 16 x u16 bone node numbers (count encoded in [u16; 16] array type)
    // 0x60..0x63: 4 bytes tail (usually 0, non-zero in ~74 vanilla models)
}

/// Offsets within an AnimMesh extra header (relative to anim extra start,
/// i.e. byte 0x19C from the node base, immediately after the TriMesh header).
///
/// Verified via `InputBinary::ResetAnim` (`0x004a0060`) and Ghidra struct
/// `MdlNodeAnimMesh` (468 bytes total).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) mod anim_mesh_offsets {
    /// Animation sampling period (f32). Extra +0x00.
    pub const SAMPLE_PERIOD: usize = 0x00;
    /// Animated vertex positions CExoArrayList pointer (u32, relocated). Extra +0x04.
    ///
    /// Points to an array of Vector (3 x f32 = 12 bytes each).
    pub const ANIM_VERTS_PTR: usize = 0x04;
    /// Number of animated vertex positions (u32, CExoArrayList.size). Extra +0x08.
    pub const ANIM_VERTS_COUNT: usize = 0x08;
    // 0x0C: CExoArrayList allocated count (ignored)
    /// Animated texture coordinates CExoArrayList pointer (u32, relocated). Extra +0x10.
    ///
    /// Points to an array of Vector (3 x f32 = 12 bytes each).
    pub const ANIM_T_VERTS_PTR: usize = 0x10;
    /// Number of animated texture coordinates (u32, CExoArrayList.size). Extra +0x14.
    pub const ANIM_T_VERTS_COUNT: usize = 0x14;
    // 0x18: CExoArrayList allocated count (ignored)

    // +0x1C..+0x37: Runtime-only fields with no ASCII parser names.
    // Verified via `InputBinary::ResetAnim` (`0x004a0060`) - relocated if
    // their size-guard counts are non-zero. `InternalParseField` (`0x0046a240`)
    // only exposes "sampleperiod", "animverts", "animtverts" - these 6 fields
    // have NO ASCII names. xoreos NWN calls the last 4 `offAnimVertices`,
    // `offAnimTextureVertices`, `verticesCount`, `textureVerticesCount`.
    //
    // Always zero in authored binary files; populated at runtime by
    // `MdlNodeAnimMesh::InternalGenVertices`. Preserved for roundtrip fidelity.

    /// Runtime data pointer 1 (u32, relocated if `data_count_1 != 0`). Extra +0x1C.
    pub const DATA_PTR_1: usize = 0x1C;
    /// Size guard for `data_ptr_1` (u32). Extra +0x20.
    pub const DATA_COUNT_1: usize = 0x20;
    /// Padding (4 bytes, untouched by Reset). Extra +0x24.
    pub const PADDING_24: usize = 0x24;
    /// Runtime animated vertices pointer (u32, relocated if count != 0). Extra +0x28.
    ///
    /// Called `offAnimVertices` by xoreos NWN.
    pub const ANIM_VERTICES_PTR: usize = 0x28;
    /// Runtime animated texture vertices pointer (u32, relocated if count != 0). Extra +0x2C.
    ///
    /// Called `offAnimTextureVertices` by xoreos NWN.
    pub const ANIM_TEX_VERTICES_PTR: usize = 0x2C;
    /// Count for `anim_vertices_ptr` (u32). Extra +0x30.
    pub const ANIM_VERTICES_COUNT: usize = 0x30;
    /// Count for `anim_tex_vertices_ptr` (u32). Extra +0x34.
    pub const ANIM_TEX_VERTICES_COUNT: usize = 0x34;
}

/// Offsets within an AABB extra header (relative to AABB extra start,
/// i.e. byte 0x19C from the node base, immediately after the TriMesh header).
///
/// Verified via `ResetMdlNode` inline processing and `ResetAABBTree`
/// (`0x004a0260`). See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) mod aabb_offsets {
    /// Root AABB tree pointer (u32, relocated). Extra +0x00.
    pub const TREE_PTR: usize = 0x00;
}

/// Offsets within a DanglyMesh extra header (relative to dangly extra start,
/// i.e. byte 0x19C from the node base, immediately after the TriMesh header).
///
/// Verified via `InputBinary::ResetDangly` (`0x004a0100`) and Ghidra struct
/// `MdlNodeDanglyMesh` (440 bytes total).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) mod dangly_offsets {
    /// Per-vertex constraint values CExoArrayList pointer (u32, relocated). Extra +0x00.
    pub const CONSTRAINTS_PTR: usize = 0x00;
    /// Number of constraint values (u32, CExoArrayList.size). Extra +0x04.
    pub const CONSTRAINTS_COUNT: usize = 0x04;
    // 0x08: CExoArrayList allocated count (ignored on read)
    /// Maximum displacement distance (f32). Extra +0x0C.
    pub const DISPLACEMENT: usize = 0x0C;
    /// Spring tightness factor (f32). Extra +0x10.
    pub const TIGHTNESS: usize = 0x10;
    /// Oscillation period (f32). Extra +0x14.
    pub const PERIOD: usize = 0x14;
    /// Per-vertex dangly positions pointer (u32, relocated if vertex_count > 0). Extra +0x18.
    ///
    /// Points to `vertex_count` x vec3 (12 bytes each) in the MDL content blob.
    /// At runtime, `PartDanglyMesh` copies these into a GL vertex pool.
    pub const DATA_PTR: usize = 0x18;
}

/// Offsets within a Saber extra header (relative to saber extra start,
/// i.e. byte 0x19C from the node base, immediately after the TriMesh header).
///
/// Verified via `InputBinary::ResetLightsaber` (`0x004a0460`).
/// kotorblender's saber reading path names these fields differently; the
/// names below are this crate's own.
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
pub(crate) mod saber_offsets {
    /// Saber vertex positions pointer (u32, relocated). Extra +0x00.
    ///
    /// Points to `NUM_SABER_VERTS` x vec3 (12 bytes each) in the MDL content blob.
    pub const VERTS_PTR: usize = 0x00;
    /// Saber texture coordinates pointer (u32, relocated). Extra +0x04.
    ///
    /// Points to `NUM_SABER_VERTS` x vec2 (8 bytes each) in the MDL content blob.
    pub const UVS_PTR: usize = 0x04;
    /// Saber normal vectors pointer (u32, relocated). Extra +0x08.
    ///
    /// Points to `NUM_SABER_VERTS` x vec3 (12 bytes each) in the MDL content blob.
    pub const NORMALS_PTR: usize = 0x08;
    /// GL vertex pool ID (runtime-only, allocated by `GLRender::RequestPool`). Extra +0x0C.
    pub const GL_POOL_VERT: usize = 0x0C;
    /// GL index pool ID (runtime-only, allocated by `GLRender::RequestPool`). Extra +0x10.
    pub const GL_POOL_INDEX: usize = 0x10;
}

/// Fixed number of saber vertices per blade mesh.
///
/// All lightsaber models use exactly 176 vertices, read from vanilla K1
/// models directly. kotorblender (`NUM_SABER_VERTS = 176`) and reone
/// (`kNumSaberSegments = 20` x `kNumSaberSegmentVertices = 4`) arrive at the
/// same figure, which corroborates the measurement rather than supplying it.
pub(crate) const NUM_SABER_VERTS: usize = 176;

/// Bitflags for node type identification in the on-disk binary format.
///
/// Node type constants verified at `0x00740a18` in `swkotor.exe`:
/// `0x01, 0x03, 0x05, 0x09, 0x11, 0x21, 0x61, 0xA1, 0x121, 0x221, 0x401, 0x821`.
/// See `docs/src/internals/mdl_deep_dive.md` "Type dispatch".
pub mod node_flags {
    /// Node has a header (always set).
    pub const HEADER: u32 = 0x0001;
    /// Node contains light data.
    pub const LIGHT: u32 = 0x0002;
    /// Node contains emitter data.
    pub const EMITTER: u32 = 0x0004;
    /// Node contains camera data.
    pub const CAMERA: u32 = 0x0008;
    /// Node is a reference to another model.
    pub const REFERENCE: u32 = 0x0010;
    /// Node contains mesh geometry.
    pub const MESH: u32 = 0x0020;
    /// Node contains skinning weights.
    pub const SKIN: u32 = 0x0040;
    /// Node contains animation data.
    pub const ANIM: u32 = 0x0080;
    /// Node contains dangly mesh physics.
    pub const DANGLY: u32 = 0x0100;
    /// Node contains an AABB tree (walkmesh).
    pub const AABB: u32 = 0x0200;
    /// Node contains saber blade data.
    pub const SABER: u32 = 0x0800;
}

/// MDL binary parsing error type.
#[derive(Debug, thiserror::Error)]
pub enum MdlError {
    /// An I/O error occurred.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// A binary layout error occurred (e.g. out of bounds).
    #[error("binary layout error: {0}")]
    Binary(#[from] rakata_formats::binary::BinaryLayoutError),

    /// The MDL header is structurally invalid.
    #[error("invalid MDL header: {0}")]
    InvalidHeader(String),

    /// The MDL data is structurally invalid.
    #[error("invalid MDL data: {0}")]
    InvalidData(String),

    /// A value exceeds the on-disk field width during writing.
    #[error("value overflow while writing field `{0}`")]
    ValueOverflow(&'static str),
}

/// A node in the MDL hierarchy.
#[derive(Debug, Clone, PartialEq)]
pub struct MdlNode {
    /// Name of the node resolved from the model's name table.
    pub name: String,
    /// Parent index (if any).
    pub parent_index: Option<u16>,
    /// Child nodes attached to this node.
    pub children: Vec<MdlNode>,
    /// Local position (x, y, z).
    pub position: [f32; 3],
    /// Local orientation quaternion (w, x, y, z).
    ///
    /// Matches the engine's `Quaternion` struct field order (Ghidra-verified).
    /// Identity quaternion is `[1.0, 0.0, 0.0, 0.0]`.
    pub rotation: [f32; 4],
    /// Type-specific node data (determines node type and carries type-specific fields).
    ///
    /// Use [`MdlNodeData::flags()`] to get the binary flags for serialization.
    pub node_data: MdlNodeData,
    /// Controllers attached to this node.
    pub controllers: Vec<MdlController>,
    /// Unreferenced controller data floats (key_count=0 but data_count>0).
    ///
    /// Some vanilla nodes (especially in supermodel combat animations) carry
    /// a controller data array with no corresponding controller key headers.
    /// The engine reads both arrays independently, so the data bytes have to
    /// be preserved for roundtrip even though no keys reference them.
    pub orphan_controller_data: Vec<f32>,
    /// Padding bytes from node header +0x02..+0x03 (between flags and node_id).
    ///
    /// Preserved verbatim for roundtrip fidelity per the reserved field rule.
    /// Zero for newly constructed nodes.
    pub header_padding_02: [u8; 2],
    /// Struct alignment padding from node header +0x06..+0x07 (2 bytes).
    ///
    /// Always zero in vanilla files. Located between the u16 name index at
    /// +0x04 and the u32 name pointer at +0x08.
    ///
    /// The fields at +0x08 (name pointer) and +0x0C (parent pointer) are
    /// relocated pointers handled by the writer; they are not stored here.
    /// See `docs/src/internals/mdl_deep_dive.md` "Base node layout".
    pub header_padding_06: [u8; 2],
}

impl MdlNode {
    /// Checks if this node contains mesh geometry (Flag 0x0020).
    pub fn is_mesh(&self) -> bool {
        self.node_data.mesh().is_some()
    }

    /// Checks if this node contains light data (Flag 0x0002).
    pub fn is_light(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Light(_))
    }

    /// Checks if this node contains emitter data (Flag 0x0004).
    pub fn is_emitter(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Emitter(_))
    }

    /// Checks if this node is a reference to another model (Flag 0x0010).
    pub fn is_reference(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Reference(_))
    }

    /// Checks if this node contains skinning weights (Flag 0x0040).
    pub fn is_skin(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Skin(_))
    }

    /// Checks if this node contains animation data (Flag 0x0080).
    pub fn is_anim(&self) -> bool {
        matches!(self.node_data, MdlNodeData::AnimMesh(_))
    }

    /// Checks if this node contains dangly mesh physics (Flag 0x0100).
    pub fn is_dangly(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Dangly(_))
    }

    /// Checks if this node contains an AABB walkmesh tree (Flag 0x0200).
    pub fn is_aabb(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Aabb(_))
    }

    /// Checks if this node contains camera data (Flag 0x0008).
    pub fn is_camera(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Camera(_))
    }

    /// Checks if this node contains saber blade data (Flag 0x0800).
    pub fn is_saber(&self) -> bool {
        matches!(self.node_data, MdlNodeData::Saber(_))
    }
}

/// A node in an animation's node tree.
///
/// Animation nodes mirror the geometry node hierarchy but carry only
/// base header data (name, node_number, controllers, children). They
/// do NOT have type-specific extra data (mesh, light, emitter, etc.).
///
/// The `node_number` field maps each animation node to its corresponding
/// geometry node so the engine can apply keyframes to the correct target.
#[derive(Debug, Clone, PartialEq)]
pub struct MdlAnimNode {
    /// Name of the node (matches a geometry node name).
    pub name: String,
    /// Node number matching the corresponding geometry node.
    pub node_number: u16,
    /// Controllers (keyframes) for this animation node.
    pub controllers: Vec<MdlController>,
    /// Unreferenced controller data floats (key_count=0 but data_count>0).
    ///
    /// See [`MdlNode::orphan_controller_data`] for rationale.
    pub orphan_controller_data: Vec<f32>,
    /// Child animation nodes.
    pub children: Vec<MdlAnimNode>,
}

/// An event that fires at a specific time during an animation.
///
/// Events trigger game logic (footstep sounds, particle effects, etc.)
/// at precise moments in the animation timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct MdlAnimEvent {
    /// Time in seconds when this event fires.
    pub time: f32,
    /// Event name (up to 32 bytes in binary, null-terminated).
    pub name: String,
}

/// A named animation sequence.
///
/// Each animation has its own node tree that mirrors (a subset of) the
/// geometry node hierarchy. The nodes carry controller keyframes that
/// animate transforms, light colors, emitter parameters, etc. over time.
///
/// The binary layout uses a 136-byte header (mirroring the geometry header
/// structure) followed by events and the animation node tree.
#[derive(Debug, Clone, PartialEq)]
pub struct MdlAnimation {
    /// Animation name (e.g. "cpause1", "walk", "attack1").
    pub name: String,
    /// Duration of the animation in seconds.
    pub length: f32,
    /// Transition time in seconds for blending into this animation.
    pub transition_time: f32,
    /// Name of the geometry node that anchors this animation.
    ///
    /// Determines which subtree of the geometry hierarchy is affected.
    /// Usually the root node name or a specific bone (e.g. "rootdummy").
    pub anim_root: String,
    /// Events that fire at specific times during playback.
    pub events: Vec<MdlAnimEvent>,
    /// Root of the animation node tree.
    pub root_node: MdlAnimNode,
    /// Function pointer 1 from the animation header.
    ///
    /// Leaked vtable pointer from the BioWare toolset.
    /// K1 PC value: `0x00413370` (4273392).
    pub fn_ptr1: u32,
    /// Function pointer 2 from the animation header.
    ///
    /// K1 PC value: `0x0043E1E0` (4451552).
    pub fn_ptr2: u32,
}

/// The high-level MDL container.
///
/// Every field is fully typed - the reader extracts all meaningful header
/// data and the writer produces correct binary output from these fields alone.
#[derive(Debug, Clone, PartialEq)]
pub struct Mdl {
    /// The root node of the model hierarchy.
    pub root_node: MdlNode,

    // --- Geometry header fields ---
    /// Function pointer 1 from the geometry header (+0x00).
    ///
    /// This is a leaked runtime vtable pointer from the BioWare toolset.
    /// Used by kotorblender for K1/K2/Xbox game detection.
    /// K1 PC value: `0x00413470`.
    pub geometry_fn_ptr1: u32,
    /// Function pointer 2 from the geometry header (+0x04).
    ///
    /// K1 PC value: `0x00405580`.
    pub geometry_fn_ptr2: u32,
    /// Model type from geometry header (+0x4C). Always 2 for geometry models.
    pub model_type: u8,

    // --- Model header fields ---
    /// Classification (0=Other, 1=Effect, 2=Tile, 4=Character, 8=Door).
    pub classification: u8,
    /// Subclassification byte (+0x51). Non-zero in ~196 vanilla K1 models.
    pub subclassification: u8,
    /// Affected-by-fog flag (+0x53). 0 or 1.
    pub affected_by_fog: u8,
    /// The supermodel name (from header, +0x88).
    pub supermodel_name: String,
    /// Total node count (from header, +0x2C).
    pub node_count: u32,
    /// Model bounding box (min_xyz, max_xyz) from header (+0x68..+0x7F).
    pub bounding_box: [f32; 6],
    /// Model bounding sphere radius from header (+0x80).
    pub radius: f32,
    /// Animation scale factor from header (+0x84). Default 1.0.
    pub animation_scale: f32,
    /// Animations attached to this model.
    pub animations: Vec<MdlAnimation>,
    /// Animation root node name, when different from the geometry root.
    ///
    /// Head models set this to `"neck_g"` so the engine applies head
    /// animations from the neck bone rather than the model root. When
    /// `None`, the writer uses the geometry root offset for +0xA8.
    pub anim_root_node: Option<String>,
    /// True when the geometry header marks this as a KotOR2 model.
    pub tsl: bool,
}

impl Mdl {
    /// True for KotOR2 (TSL) models.
    pub fn is_tsl(&self) -> bool {
        self.tsl
    }
}

impl DecodeBinary for Mdl {
    type Error = MdlError;

    /// Decodes an MDL model from raw bytes without MDX vertex data.
    ///
    /// For models with companion MDX data, use [`read_mdl_from_bytes`] directly
    /// with the `mdx_bytes` parameter.
    fn decode_binary(bytes: &[u8]) -> Result<Self, Self::Error> {
        read_mdl_from_bytes(bytes, None)
    }
}

/// Assigns the per-mesh sequence value to every mesh node in DFS tree order.
///
/// The value is stored at CExoArrayList offset `+0xC8`. Its shape is a set of
/// checkpoints at `100 x 2^k`, where the value equals the counter, separated
/// by runs that descend between them.
///
/// ```text
/// // c is the 1-based mesh counter
/// let mut t = 100;
/// while t < c { t *= 2; }
/// match c {
///     _ if c == t => c,          // checkpoint
///     _ if t == 100 => 99 - c,   // first run, the special case
///     _ => t + t / 2 - c,
/// }
/// ```
///
/// Derived from the retail corpus rather than borrowed. It reproduces every
/// stored value on the deepest unambiguous model and on every model whose
/// counter is recoverable, it is injective over the range tested, and it
/// never emits `99` -- the first run descends to zero and the first
/// checkpoint is `100`, so that gap is real rather than a rounding artefact.
///
/// mdledit's formula is not this one. It agrees up to counter 299 and is
/// wrong at every counter after, which only shows on models large enough to
/// reach 300.
///
/// **Saber meshes are skipped entirely.** They write none of the three
/// single-`u32` blocks this value belongs to, so they take no counter and
/// consume no increment.
///
/// This is for newly constructed models. Binary-parsed models keep the value
/// the file carried, which matters because the mapping from mesh ordinal to
/// counter is **not recoverable from the file** -- see
/// `docs/src/formats/models/mesh_derived_fields.md` "1.5 index_buffer_pools /
/// Inverted Counter".
pub fn assign_inverted_counters(root: &mut MdlNode) {
    fn compute_inverted(counter: u32) -> u32 {
        let mut checkpoint = 100u32;
        while checkpoint < counter {
            checkpoint = checkpoint.saturating_mul(2);
        }
        if counter == checkpoint {
            counter
        } else if checkpoint == 100 {
            // The first run is the special case: it descends 98..0 and the
            // value 99 is never produced.
            99 - counter
        } else {
            checkpoint + checkpoint / 2 - counter
        }
    }

    fn walk(node: &mut MdlNode, counter: &mut u32) {
        // A saber mesh writes none of the blocks this value indexes, so it
        // takes no counter and advances nothing.
        let is_saber = matches!(node.node_data, types::MdlNodeData::Saber(_));
        if !is_saber {
            if let Some(mesh) = node.node_data.mesh_mut() {
                *counter += 1;
                mesh.inverted_counter = compute_inverted(*counter);
            }
        }
        for child in &mut node.children {
            walk(child, counter);
        }
    }

    let mut counter = 0u32;
    walk(root, &mut counter);
}

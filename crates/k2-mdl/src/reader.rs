//! MDL binary reader.
//!
//! The reader uses a two-pass architecture:
//!
//! **Pass 1 (node tree):** Recursive DFS traversal of the node hierarchy. Each
//! node's headers and type-specific extra data are parsed, and mesh nodes
//! collect an `MdxMeshInfo` descriptor with stride, vertex count, and
//! per-attribute byte offsets. Vertex attribute arrays are left empty at this
//! stage - only structural metadata is captured.
//!
//! **Pass 2 (MDX vertex population):** After the full tree is built, vertex
//! data is populated from the MDX buffer (or from MDL content-blob fallback
//! positions when no MDX is present). Meshes are visited in non-skin-first
//! order - all non-skin meshes in DFS order, then all skin meshes in DFS
//! order - matching the BioWare engine's canonical MDX layout. A cumulative
//! cursor advances through the MDX buffer, reading interleaved vertex
//! attributes and skipping terminator/alignment padding between meshes.
//!
//! This two-pass design decouples the node parse order (DFS) from the MDX
//! data order (non-skin-first), enabling correct vertex assignment for vanilla
//! game files where these orders differ.

use std::io::{Cursor, Read, Seek, SeekFrom};

use rakata_formats::binary::{
    check_slice_in_bounds, checked_to_usize, read_f32, read_i32, read_u16, read_u32, read_u8,
};

use super::controllers::{MdlController, MdlControllerType, MdlKey};
use super::types::{
    AabbNode, MdlAabb, MdlAnimMesh, MdlCamera, MdlDangly, MdlLight, MdlMesh, MdlNodeData,
    MdlReference, MdlSaber, MdlSkin,
};
use super::{
    aabb_offsets, anim_header_offsets, anim_mesh_offsets, dangly_offsets, header_offsets,
    light_offsets, mesh_offsets, node_flags, node_offsets, saber_offsets, skin_offsets, Mdl,
    MdlAnimEvent, MdlAnimNode, MdlAnimation, MdlError, MdlNode, AABB_EXTRA_SIZE,
    ANIMATION_EVENT_SIZE, ANIMATION_HEADER_SIZE, ANIM_MESH_EXTRA_SIZE, DANGLY_EXTRA_SIZE,
    EMITTER_EXTRA_SIZE, LIGHT_EXTRA_SIZE, MAX_FACE_SIZE, MDL_WRAPPER_SIZE, MESH_EXTRA_SIZE,
    NODE_HEADER_SIZE, NUM_SABER_VERTS, REFERENCE_EXTRA_SIZE, SABER_EXTRA_SIZE, SKIN_EXTRA_SIZE,
    TSL_MESH_EXTRA,
};

/// Reads an MDL file from the given reader.
///
/// This buffers the entire stream to memory.
///
/// # Errors
///
/// [`MdlError::Io`] when the stream will not read to end, and whatever
/// [`read_mdl_from_bytes`] reports for the bytes it collected.
pub fn read_mdl<R: Read>(reader: &mut R, mdx_bytes: Option<&[u8]>) -> Result<Mdl, MdlError> {
    let mut buffer = Vec::new();
    reader.read_to_end(&mut buffer)?;
    crate::trace_debug!(bytes_len = buffer.len(), "read mdl bytes from reader");
    read_mdl_from_bytes(&buffer, mdx_bytes)
}

/// Reads an MDL file from the given byte slice.
///
/// Accepts an optional `mdx_bytes` slice for external geometry data.
/// This currently supports reading the node hierarchy, names, transforms,
/// and basic mesh headers (vertex counts).
///
/// # Errors
///
/// [`MdlError::InvalidHeader`] when `bytes` are shorter than the MDL header or
/// a declared section runs past the end, [`MdlError::InvalidData`] when the
/// node tree does not hang together, [`MdlError::ValueOverflow`] when a
/// declared offset or count will not fit `usize`, and [`MdlError::Binary`]
/// when a primitive read runs off the end.
///
/// An absent `mdx_bytes` is not an error: the model reads without vertex data,
/// which is what a caller inspecting node structure wants. An MDX that is
/// present and too short truncates the affected mesh rather than failing, the
/// way vanilla K1 item models sharing one MDX buffer require.
pub fn read_mdl_from_bytes(bytes: &[u8], mdx_bytes: Option<&[u8]>) -> Result<Mdl, MdlError> {
    read_mdl_from_cursor(&mut Cursor::new(bytes), mdx_bytes)
}

// ---------------------------------------------------------------------------
// Two-pass MDX reading infrastructure
// ---------------------------------------------------------------------------

/// Metadata collected during Pass 1 (DFS tree parse) for deferred MDX reading.
///
/// During Pass 1, the tree is built with empty vertex arrays on every mesh.
/// Each mesh's MDX-relevant metadata is stored here so that Pass 2 can read
/// vertex data in the correct ordering (non-skin-first, matching the writer).
struct MdxMeshInfo {
    /// Per-vertex byte stride in the MDX data.
    stride: u32,
    /// Number of vertices declared in the mesh header.
    vertex_count: usize,
    /// Per-mesh offset into the MDX file (mesh header +0x144).
    /// Used by the engine and community tools to seek to the correct
    /// position in the MDX buffer for each mesh's interleaved vertex data.
    mdx_data_offset: usize,
    /// Content-relative pointer to position-only vertex data (mesh header
    /// +0x148). Points to vertex_count * 12 bytes (3x f32) in the MDL
    /// content blob. Used as fallback when no MDX file is available.
    vert_array_offset: usize,
    /// Per-attribute byte offsets within the interleaved stride (-1 = absent).
    pos_off: i32,
    norm_off: i32,
    color_off: i32,
    uv1_off: i32,
    uv2_off: i32,
    uv3_off: i32,
    uv4_off: i32,
    tangent_off: i32,
    /// Bone weight/index byte offsets (skin meshes only, -1 when absent).
    bone_weights_off: i32,
    bone_indices_off: i32,
}

/// Vertex data read from MDX for a single mesh, ready to be applied to the tree.
#[derive(Default)]
struct MdxVertexData {
    positions: Vec<[f32; 3]>,
    normals: Vec<[f32; 3]>,
    vertex_colors: Vec<[u8; 4]>,
    uv1: Vec<[f32; 2]>,
    uv2: Vec<[f32; 2]>,
    uv3: Vec<[f32; 2]>,
    uv4: Vec<[f32; 2]>,
    tangent_space: Vec<[[f32; 3]; 3]>,
    bone_weights: Vec<[f32; 4]>,
    bone_indices: Vec<[f32; 4]>,
    /// If the MDX data was truncated, the clamped vertex count.
    clamped_vertex_count: Option<u16>,
}

// ---------------------------------------------------------------------------
// Shared reader helpers
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Top-level reader
// ---------------------------------------------------------------------------

fn read_mdl_from_cursor<R: Read + Seek>(
    reader: &mut R,
    mdx_bytes: Option<&[u8]>,
) -> Result<Mdl, MdlError> {
    // 1. Skip 12-byte preamble/wrapper.
    let len = reader.seek(SeekFrom::End(0))?;
    if len < MDL_WRAPPER_SIZE {
        return Err(rakata_formats::binary::BinaryLayoutError::UnexpectedEof("preamble").into());
    }
    reader.seek(SeekFrom::Start(MDL_WRAPPER_SIZE))?;

    // Read remaining content (treating byte 12 as offset 0)
    let mut content = Vec::new();
    reader.read_to_end(&mut content)?;
    let bytes = &content;

    // 2. Parse Header (196 bytes: 80-byte geometry header + 116-byte model header)
    if bytes.len() < 196 {
        return Err(rakata_formats::binary::BinaryLayoutError::UnexpectedEof("header").into());
    }

    // --- Geometry header fields ---
    let geometry_fn_ptr1 = read_u32(bytes, header_offsets::FN_PTR1)?;
    let geometry_fn_ptr2 = read_u32(bytes, header_offsets::FN_PTR2)?;
    let tsl = matches!(
        geometry_fn_ptr1,
        super::game_fn_ptr::K2_PC | super::game_fn_ptr::K2_XBOX
    );
    let root_node_offset = checked_to_usize(
        read_u32(bytes, header_offsets::ROOT_NODE_PTR)?,
        "root_offset",
    )?;
    let node_count = read_u32(bytes, header_offsets::NODE_COUNT)?;
    let model_type = read_u8(bytes, header_offsets::MODEL_TYPE)?;

    // --- Model header fields ---
    let classification = read_u8(bytes, header_offsets::CLASSIFICATION)?;
    let subclassification = read_u8(bytes, header_offsets::SUBCLASSIFICATION)?;
    let affected_by_fog = read_u8(bytes, header_offsets::AFFECTED_BY_FOG)?;

    // Bounding box: 6 floats (min_xyz, max_xyz)
    let mut bounding_box = [0.0f32; 6];
    for (i, val) in bounding_box.iter_mut().enumerate() {
        *val = read_f32(bytes, header_offsets::BOUNDING_BOX_MIN + i * 4)?;
    }
    let radius = read_f32(bytes, header_offsets::RADIUS)?;
    let animation_scale = read_f32(bytes, header_offsets::ANIMATION_SCALE)?;

    // Supermodel name: null-terminated string at +0x88, up to 32 bytes.
    let supermodel_name = read_fixed_string(
        bytes,
        header_offsets::SUPERMODEL_NAME,
        header_offsets::SUPERMODEL_NAME_SIZE,
    );

    let name_offsets_ptr = checked_to_usize(
        read_u32(bytes, header_offsets::NAME_OFFSETS_PTR)?,
        "name_offsets",
    )?;
    let name_count = read_u32(bytes, header_offsets::NAME_COUNT)?;

    // 3. Name Resolution
    let mut names = Vec::new();
    if name_offsets_ptr > 0 && name_offsets_ptr < bytes.len() {
        let name_count = checked_to_usize(name_count, "name_count")?;
        for i in 0..name_count {
            let ptr_offset = name_offsets_ptr + (i * 4);
            if ptr_offset + 4 > bytes.len() {
                crate::trace_warn!(
                    index = i,
                    ptr_offset,
                    bytes_len = bytes.len(),
                    "name offset array truncated"
                );
                break;
            }

            let name_offset = checked_to_usize(read_u32(bytes, ptr_offset)?, "name_ptr")?;

            if name_offset < bytes.len() {
                names.push(rakata_formats::binary::read_c_string(bytes, name_offset));
            } else {
                crate::trace_warn!(
                    index = i,
                    name_offset,
                    bytes_len = bytes.len(),
                    "name string offset out of bounds, using synthetic name"
                );
                names.push(format!("Node_{}", i));
            }
        }
    }

    // 4. Pass 1: Recursive tree traversal (no MDX reading).
    // Builds the complete node tree with empty vertex arrays on meshes.
    // MDX metadata is collected into `mdx_infos` for Pass 2.
    let mut mdx_infos: Vec<MdxMeshInfo> = Vec::new();
    let mut dfs_mesh_counter: usize = 0;
    let mut root = read_node(
        bytes,
        root_node_offset,
        &names,
        None,
        None,
        &mut mdx_infos,
        &mut dfs_mesh_counter,
        tsl,
    )?;

    // 5. Pass 2: Populate vertex data from MDX or MDL content.
    //
    // MDX vertex data is laid out in non-skin-first order (BioWare vanilla
    // convention): all non-skin meshes first in DFS order, then all skin
    // meshes in DFS order. The writer uses this same ordering.
    //
    // TODO: MDX ordering auto-detection for modded files.
    //
    // Currently assumes non-skin-first ordering (matches BioWare vanilla).
    // Files produced by mdlops (DFS order) or PyKotor (node-ID order) with
    // mixed skin/non-skin mesh nodes will get vertex data misassigned.
    //
    // Future: detect ordering via sentinel float scanning - check for 10M/1M
    // sentinel values at expected positions for each candidate ordering, pick
    // the one that matches. See vanilla-inspector's `diagnose_mdx_strategies()`
    // for the 10-strategy approach.
    if let Some(mdx) = mdx_bytes {
        populate_mdx_data(&mut root, mdx, &mdx_infos)?;
        // Fallback: mesh nodes that the MDX pass skipped (stride=0, e.g.,
        // saber nodes) still need their embedded content positions read from
        // vert_array_offset (+0x148). Without this, the writer would emit
        // vert_array_offset=0 and the engine loses the position fallback.
        populate_content_positions_fallback(&mut root, bytes, &mdx_infos)?;
    } else {
        populate_content_positions(&mut root, bytes, &mdx_infos)?;
    }

    // 6. Parse Animations
    let anim_arr_ptr = read_u32(bytes, header_offsets::ANIMATION_ARR_PTR)?;
    let anim_arr_count = read_u32(bytes, header_offsets::ANIMATION_ARR_COUNT)?;
    let animations = read_animations(bytes, anim_arr_ptr, anim_arr_count, &names)?;

    // 7. Resolve off_anim_root (+0xA8).
    // Head models point this to `neck_g` instead of the geometry root.
    let off_anim_root_raw = read_u32(bytes, header_offsets::OFF_ANIM_ROOT)?;
    let anim_root_node = if off_anim_root_raw
        != u32::try_from(root_node_offset)
            .map_err(|_| MdlError::ValueOverflow("root_node_offset"))?
        && off_anim_root_raw > 0
    {
        let ar_offset = checked_to_usize(off_anim_root_raw, "off_anim_root")?;
        if ar_offset + 6 <= bytes.len() {
            let name_idx = usize::from(read_u16(bytes, ar_offset + node_offsets::NAME_INDEX)?);
            if name_idx < names.len() {
                Some(names[name_idx].clone())
            } else {
                None
            }
        } else {
            None
        }
    } else {
        None
    };

    Ok(Mdl {
        root_node: root,
        geometry_fn_ptr1,
        geometry_fn_ptr2,
        model_type,
        classification,
        subclassification,
        affected_by_fog,
        supermodel_name,
        node_count,
        bounding_box,
        radius,
        animation_scale,
        animations,
        anim_root_node,
        tsl,
    })
}

// ---------------------------------------------------------------------------
// Shared controller reader
// ---------------------------------------------------------------------------

/// Reads controller key headers and their associated keyframe data.
///
/// Used by both geometry nodes and animation nodes - the binary format
/// is identical. Each controller key header is 16 bytes.
/// Result of reading controller arrays from a node header.
///
/// Contains both parsed controllers (when key_count > 0) and any orphan data
/// floats (when key_count == 0 but data_count > 0).
struct ControllerReadResult {
    controllers: Vec<MdlController>,
    orphan_data: Vec<f32>,
}

fn read_controllers(
    bytes: &[u8],
    key_ptr: usize,
    key_count: usize,
    data_ptr: usize,
    data_count: usize,
) -> Result<ControllerReadResult, MdlError> {
    // Read controller data floats first.
    let mut controller_data = Vec::with_capacity(data_count);
    if data_count > 0 && data_ptr > 0 {
        check_slice_in_bounds(bytes, data_ptr, data_count * 4, "controller_data")?;
        for i in 0..data_count {
            let val = read_f32(bytes, data_ptr + (i * 4))?;
            controller_data.push(val);
        }
    }

    let mut controllers = Vec::with_capacity(key_count);
    if key_count > 0 && key_ptr > 0 {
        for i in 0..key_count {
            let key_offset = key_ptr + (i * 16);
            if key_offset + 16 > bytes.len() {
                crate::trace_warn!(
                    index = i,
                    key_offset,
                    bytes_len = bytes.len(),
                    "controller key header truncated"
                );
                break;
            }

            let type_code = read_u32(bytes, key_offset)?;
            let controller_type = MdlControllerType::from(type_code);

            // Preserve unknown bytes for roundtrip fidelity (reserved field rule).
            let mut key_unknown_04 = [0u8; 2];
            key_unknown_04.copy_from_slice(&bytes[key_offset + 4..key_offset + 6]);

            let row_count = usize::from(read_u16(bytes, key_offset + 6)?);
            let time_index = usize::from(read_u16(bytes, key_offset + 8)?);
            let data_index = usize::from(read_u16(bytes, key_offset + 10)?);
            let raw_column_count = read_u8(bytes, key_offset + 12)?;

            let mut key_unknown_0d = [0u8; 3];
            key_unknown_0d.copy_from_slice(&bytes[key_offset + 13..key_offset + 16]);

            // Decode column_count to determine actual values per keyframe row.
            //
            // The raw byte encodes:
            // - Lower 4 bits: base column count
            // - Bit 4 (0x10): Bezier flag (3x control point triplets)
            // - Special case: ORIENTATION with raw == 2 means integral
            //     compressed quaternion (1 u32 per row, stored as f32 bits)
            //
            // See kotorblender reader.py load_controllers() for reference.
            let actual_columns =
                if controller_type == MdlControllerType::ORIENTATION && raw_column_count == 2 {
                    // Integral compressed quaternion: 1 packed u32 per row.
                    // Read as f32, bit pattern preserved for roundtrip.
                    1
                } else {
                    let base = usize::from(raw_column_count & 0x0F);
                    let has_bezier = (raw_column_count & super::controllers::CTRL_FLAG_BEZIER) != 0;
                    if has_bezier {
                        base * 3
                    } else {
                        base
                    }
                };

            // Reconstruct keyframe rows.
            let mut keys = Vec::with_capacity(row_count);

            if time_index + row_count <= controller_data.len() {
                for r in 0..row_count {
                    let time = controller_data[time_index + r];
                    let mut values = Vec::with_capacity(actual_columns);

                    let data_start = data_index + (r * actual_columns);
                    if data_start + actual_columns <= controller_data.len() {
                        for c in 0..actual_columns {
                            values.push(controller_data[data_start + c]);
                        }
                    } else {
                        crate::trace_warn!(
                            controller = ?controller_type,
                            row = r,
                            data_start,
                            actual_columns,
                            raw_column_count,
                            data_len = controller_data.len(),
                            "controller keyframe data out of bounds"
                        );
                    }
                    keys.push(MdlKey { time, values });
                }
            } else {
                crate::trace_warn!(
                    controller = ?controller_type,
                    time_index,
                    row_count,
                    data_len = controller_data.len(),
                    "controller time indices out of bounds"
                );
            }

            controllers.push(MdlController {
                controller_type,
                keys,
                raw_column_count,
                key_unknown_04,
                key_unknown_0d,
            });
        }
    }

    // When key_count == 0 but data was read, preserve as orphan data.
    let orphan_data = if controllers.is_empty() && !controller_data.is_empty() {
        controller_data
    } else {
        Vec::new()
    };

    Ok(ControllerReadResult {
        controllers,
        orphan_data,
    })
}

// ---------------------------------------------------------------------------
// Animation reader
// ---------------------------------------------------------------------------

/// Reads all animations from the animation offset array.
fn read_animations(
    bytes: &[u8],
    anim_arr_ptr: u32,
    anim_arr_count: u32,
    names: &[String],
) -> Result<Vec<MdlAnimation>, MdlError> {
    if anim_arr_count == 0 {
        return Ok(Vec::new());
    }

    let arr_offset = checked_to_usize(anim_arr_ptr, "anim_arr_ptr")?;
    let anim_count = checked_to_usize(anim_arr_count, "anim_count")?;
    if arr_offset + (anim_count * 4) > bytes.len() {
        return Err(MdlError::InvalidData(format!(
            "animation offset array at {arr_offset} with {anim_arr_count} entries exceeds content"
        )));
    }

    // Read the array of content-relative offsets to animation headers.
    let mut anim_offsets = Vec::with_capacity(anim_count);
    for i in 0..anim_count {
        let off = checked_to_usize(read_u32(bytes, arr_offset + i * 4)?, "anim_header_offset")?;
        anim_offsets.push(off);
    }

    let mut animations = Vec::with_capacity(anim_offsets.len());
    for &offset in &anim_offsets {
        animations.push(read_animation(bytes, offset, names)?);
    }
    Ok(animations)
}

/// Reads a single animation from its header offset.
fn read_animation(bytes: &[u8], offset: usize, names: &[String]) -> Result<MdlAnimation, MdlError> {
    check_slice_in_bounds(bytes, offset, ANIMATION_HEADER_SIZE, "animation_header")?;

    let fn_ptr1 = read_u32(bytes, offset + anim_header_offsets::FN_PTR1)?;
    let fn_ptr2 = read_u32(bytes, offset + anim_header_offsets::FN_PTR2)?;

    // Animation name (32-byte null-terminated).
    let name = read_fixed_string(
        bytes,
        offset + anim_header_offsets::NAME,
        anim_header_offsets::NAME_SIZE,
    );

    let root_node_ptr = checked_to_usize(
        read_u32(bytes, offset + anim_header_offsets::ROOT_NODE_PTR)?,
        "anim_root_node_ptr",
    )?;

    // length and transition_time
    let length = read_f32(bytes, offset + anim_header_offsets::LENGTH)?;
    let transition_time = read_f32(bytes, offset + anim_header_offsets::TRANSITION)?;

    // Animation root name (32-byte null-terminated).
    let anim_root = read_fixed_string(
        bytes,
        offset + anim_header_offsets::ANIM_ROOT,
        anim_header_offsets::ANIM_ROOT_SIZE,
    );

    // Event array
    let event_arr_ptr = read_u32(bytes, offset + anim_header_offsets::EVENT_ARR_PTR)?;
    let event_arr_count = read_u32(bytes, offset + anim_header_offsets::EVENT_ARR_COUNT)?;

    let events = read_anim_events(bytes, event_arr_ptr, event_arr_count)?;

    // Parse animation node tree recursively.
    let root_node = read_anim_node(bytes, root_node_ptr, names)?;

    Ok(MdlAnimation {
        name,
        length,
        transition_time,
        anim_root,
        events,
        root_node,
        fn_ptr1,
        fn_ptr2,
    })
}

/// Reads animation events from the event array.
fn read_anim_events(
    bytes: &[u8],
    event_arr_ptr: u32,
    event_arr_count: u32,
) -> Result<Vec<MdlAnimEvent>, MdlError> {
    if event_arr_count == 0 {
        return Ok(Vec::new());
    }

    let arr_offset = checked_to_usize(event_arr_ptr, "event_arr_ptr")?;
    let event_count = checked_to_usize(event_arr_count, "event_count")?;
    let total_size = event_count * ANIMATION_EVENT_SIZE;
    if arr_offset + total_size > bytes.len() {
        return Err(MdlError::InvalidData(format!(
            "event array at {arr_offset} with {event_arr_count} events exceeds content"
        )));
    }

    let mut events = Vec::with_capacity(event_count);
    for i in 0..event_count {
        let event_offset = arr_offset + i * ANIMATION_EVENT_SIZE;
        let time = read_f32(bytes, event_offset)?;
        let name = read_fixed_string(bytes, event_offset + 4, 32);
        events.push(MdlAnimEvent { time, name });
    }
    Ok(events)
}

/// Reads a single animation node and its children recursively.
///
/// Animation nodes use the same 80-byte base header layout as geometry nodes
/// but carry no type-specific extra data. The `node_number` field maps back
/// to the corresponding geometry node.
fn read_anim_node(bytes: &[u8], offset: usize, names: &[String]) -> Result<MdlAnimNode, MdlError> {
    check_slice_in_bounds(bytes, offset, NODE_HEADER_SIZE, "anim_node_header")?;

    // +0x02: node_number - maps this animation node to its geometry counterpart.
    let node_number = read_u16(bytes, offset + 0x02)?;

    // +0x04: name_index - index into the model's name table.
    let name_index = usize::from(read_u16(bytes, offset + node_offsets::NAME_INDEX)?);
    let name = if name_index < names.len() {
        names[name_index].clone()
    } else {
        format!("AnimNode_{}", name_index)
    };

    // Children array.
    let child_offset_ptr = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CHILD_ARRAY_PTR)?,
        "anim_child_offset_ptr",
    )?;
    let child_count = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CHILD_COUNT)?,
        "anim_child_count",
    )?;

    // Controller arrays.
    let ctrl_key_ptr = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_KEY_PTR)?,
        "anim_ctrl_key_ptr",
    )?;
    let ctrl_key_count = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_KEY_COUNT)?,
        "anim_ctrl_key_count",
    )?;
    let ctrl_data_ptr = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_DATA_PTR)?,
        "anim_ctrl_data_ptr",
    )?;
    let ctrl_data_count = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_DATA_COUNT)?,
        "anim_ctrl_data_count",
    )?;

    // Parse controllers (reusing same infrastructure as geometry nodes).
    let ctrl_result = read_controllers(
        bytes,
        ctrl_key_ptr,
        ctrl_key_count,
        ctrl_data_ptr,
        ctrl_data_count,
    )?;

    // Parse children recursively.
    let mut children = Vec::with_capacity(child_count);
    if child_count > 0 && child_offset_ptr > 0 && child_offset_ptr + child_count * 4 <= bytes.len()
    {
        for i in 0..child_count {
            let child_ptr =
                checked_to_usize(read_u32(bytes, child_offset_ptr + i * 4)?, "anim_child_ptr")?;
            children.push(read_anim_node(bytes, child_ptr, names)?);
        }
    }

    Ok(MdlAnimNode {
        name,
        node_number,
        controllers: ctrl_result.controllers,
        orphan_controller_data: ctrl_result.orphan_data,
        children,
    })
}

// ---------------------------------------------------------------------------
// Geometry node reader
// ---------------------------------------------------------------------------

fn read_node(
    bytes: &[u8],
    offset: usize,
    names: &[String],
    parent_node_id: Option<u16>,
    node_end_hint: Option<usize>,
    mdx_infos: &mut Vec<MdxMeshInfo>,
    dfs_mesh_counter: &mut usize,
    tsl: bool,
) -> Result<MdlNode, MdlError> {
    // Basic Node Header check (0x44 = 68 bytes)
    check_slice_in_bounds(bytes, offset, NODE_HEADER_SIZE, "node_header")?;

    let flags = u32::from(read_u16(bytes, offset + node_offsets::FLAGS)?);

    // Preserve unknown bytes for roundtrip fidelity (reserved field rule).
    let mut header_padding_02 = [0u8; 2];
    header_padding_02.copy_from_slice(&bytes[offset + 0x02..offset + 0x04]);

    let name_index = usize::from(read_u16(bytes, offset + node_offsets::NAME_INDEX)?);

    let mut header_padding_06 = [0u8; 2];
    header_padding_06.copy_from_slice(&bytes[offset + 0x06..offset + 0x08]);

    let px = read_f32(bytes, offset + node_offsets::POS_X)?;
    let py = read_f32(bytes, offset + node_offsets::POS_X + 4)?;
    let pz = read_f32(bytes, offset + node_offsets::POS_X + 8)?;

    // Orientation quaternion (w, x, y, z) - Ghidra-verified field order
    let rw = read_f32(bytes, offset + node_offsets::ORIENTATION_W)?;
    let rx = read_f32(bytes, offset + node_offsets::ORIENTATION_W + 4)?;
    let ry = read_f32(bytes, offset + node_offsets::ORIENTATION_W + 8)?;
    let rz = read_f32(bytes, offset + node_offsets::ORIENTATION_W + 12)?;

    let child_offset_ptr = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CHILD_ARRAY_PTR)?,
        "child_offset",
    )?;
    let child_count = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CHILD_COUNT)?,
        "child_count",
    )?;

    let controller_key_ptr = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_KEY_PTR)?,
        "controller_key_ptr",
    )?;
    let controller_key_count = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_KEY_COUNT)?,
        "controller_key_count",
    )?;

    let controller_data_ptr = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_DATA_PTR)?,
        "controller_data_ptr",
    )?;
    let controller_data_count = checked_to_usize(
        read_u32(bytes, offset + node_offsets::CONTROLLER_DATA_COUNT)?,
        "controller_data_count",
    )?;

    let name = if name_index < names.len() {
        names[name_index].clone()
    } else {
        format!("Node_{}", name_index)
    };

    // Read controllers using shared helper.
    let ctrl_result = read_controllers(
        bytes,
        controller_key_ptr,
        controller_key_count,
        controller_data_ptr,
        controller_data_count,
    )?;

    // Sequential Header Parsing
    // Standard packing order: Light -> Emitter -> Camera -> Reference -> Mesh -> Skin
    // WARNING: This assumes fixed sizes which is brittle. Real parser needs exact struct sizes.
    let mut current_offset = offset + NODE_HEADER_SIZE; // End of Node Header

    if (flags & node_flags::LIGHT) != 0 {
        current_offset += LIGHT_EXTRA_SIZE;
    }
    if (flags & node_flags::EMITTER) != 0 {
        current_offset += EMITTER_EXTRA_SIZE;
    }
    // Camera: 0 extra bytes beyond the base node header (Ghidra-verified,
    // see mdl_mdx.md: Non-Mesh Node Type Structs).
    // No offset adjustment needed.
    if (flags & node_flags::REFERENCE) != 0 {
        current_offset += REFERENCE_EXTRA_SIZE;
    }

    // Determine node_data variant from flags.
    // Priority: check mesh subtypes first (most specific), then non-mesh types,
    // then plain mesh, then base.
    let node_data = if (flags & node_flags::MESH) != 0 {
        *dfs_mesh_counter += 1;

        let mut node_data_end_hint = node_end_hint.unwrap_or(bytes.len());
        for ptr in [child_offset_ptr, controller_key_ptr, controller_data_ptr] {
            if ptr > 0 {
                node_data_end_hint = node_data_end_hint.min(ptr);
            }
        }

        let (mesh, info) = read_mesh_header(bytes, current_offset, tsl)?;
        mdx_infos.push(info);
        let mesh_extra_size = MESH_EXTRA_SIZE + if tsl { TSL_MESH_EXTRA } else { 0 };

        if (flags & node_flags::SABER) != 0 {
            let saber = read_saber_extra(bytes, current_offset + mesh_extra_size, mesh)?;
            MdlNodeData::Saber(saber)
        } else if (flags & node_flags::AABB) != 0 {
            let aabb = read_aabb_extra(
                bytes,
                current_offset + mesh_extra_size,
                node_data_end_hint,
                mesh,
            )?;
            MdlNodeData::Aabb(aabb)
        } else if (flags & node_flags::DANGLY) != 0 {
            let dangly = read_dangly_extra(bytes, current_offset + mesh_extra_size, mesh)?;
            MdlNodeData::Dangly(dangly)
        } else if (flags & node_flags::ANIM) != 0 {
            let anim = read_anim_mesh_extra(bytes, current_offset + mesh_extra_size, mesh)?;
            MdlNodeData::AnimMesh(anim)
        } else if (flags & node_flags::SKIN) != 0 {
            let skin = read_skin_extra(bytes, current_offset + mesh_extra_size, mesh)?;
            // Propagate bone weight/index offsets into the MdxMeshInfo so the
            // MDX vertex reader can extract typed bone data.
            if let Some(last_info) = mdx_infos.last_mut() {
                last_info.bone_weights_off = skin.mdx_bone_weights_offset;
                last_info.bone_indices_off = skin.mdx_bone_indices_offset;
            }
            MdlNodeData::Skin(skin)
        } else {
            MdlNodeData::Mesh(mesh)
        }
    } else if (flags & node_flags::LIGHT) != 0 {
        MdlNodeData::Light(read_light_header(bytes, offset + NODE_HEADER_SIZE)?)
    } else if (flags & node_flags::EMITTER) != 0 {
        MdlNodeData::Emitter(read_emitter_header(bytes, offset + NODE_HEADER_SIZE)?)
    } else if (flags & node_flags::CAMERA) != 0 {
        MdlNodeData::Camera(MdlCamera::new())
    } else if (flags & node_flags::REFERENCE) != 0 {
        MdlNodeData::Reference(read_reference_header(bytes, offset + NODE_HEADER_SIZE)?)
    } else {
        MdlNodeData::Base
    };

    let mut children = Vec::with_capacity(child_count);
    if child_count > 0 && child_offset_ptr > 0 {
        let mut child_ptrs = Vec::with_capacity(child_count);
        for i in 0..child_count {
            let ptr_loc = child_offset_ptr + (i * 4);
            let child_ptr = checked_to_usize(read_u32(bytes, ptr_loc)?, "child_ptr")?;
            child_ptrs.push(child_ptr);
        }

        for (i, child_ptr) in child_ptrs.iter().copied().enumerate() {
            let mut child_end_hint: Option<usize> = None;
            let mut consider_end = |candidate: Option<usize>| {
                if let Some(end) = candidate.filter(|end| *end > child_ptr) {
                    child_end_hint = Some(child_end_hint.map_or(end, |curr| curr.min(end)));
                }
            };
            consider_end(child_ptrs.get(i + 1).copied());
            consider_end((controller_key_ptr > 0).then_some(controller_key_ptr));
            consider_end((controller_data_ptr > 0).then_some(controller_data_ptr));
            consider_end(node_end_hint);

            let child_node = read_node(
                bytes,
                child_ptr,
                names,
                u16::try_from(name_index).ok(),
                child_end_hint,
                mdx_infos,
                dfs_mesh_counter,
                tsl,
            )?;
            children.push(child_node);
        }
    }

    Ok(MdlNode {
        name,
        parent_index: parent_node_id,
        children,
        position: [px, py, pz],
        rotation: [rw, rx, ry, rz],
        node_data,
        controllers: ctrl_result.controllers,
        orphan_controller_data: ctrl_result.orphan_data,
        header_padding_02,
        header_padding_06,
    })
}

// ---------------------------------------------------------------------------
// Mesh header (Pass 1 - no MDX reading)
// ---------------------------------------------------------------------------

fn read_mesh_header(
    bytes: &[u8],
    offset: usize,
    tsl: bool,
) -> Result<(MdlMesh, MdxMeshInfo), MdlError> {
    use super::types::{MdlFace, MdlMeshTsl};

    // Full mesh extra header is 332 bytes (MdlNodeTriMesh 0x50..0x19C) in K1,
    // 340 in K2.
    let k2 = if tsl { TSL_MESH_EXTRA } else { 0 };
    check_slice_in_bounds(bytes, offset, MESH_EXTRA_SIZE + k2, "mesh_header")?;

    // K2 inserts dirt/hologram fields after `render` (+0x139):
    // dirt_enabled u8, pad, dirt_texture u16, dirt_coord_space u16,
    // hide_in_holograms u8, pad. Fields from total_area on shift by +8.
    let tsl_fields = if tsl {
        let base = offset + mesh_offsets::RENDER + 1;
        Some(MdlMeshTsl {
            dirt_enabled: read_u8(bytes, base)? != 0,
            dirt_texture: read_u16(bytes, base + 2)?,
            dirt_coord_space: read_u16(bytes, base + 4)?,
            hide_in_holograms: read_u8(bytes, base + 6)? != 0,
        })
    } else {
        None
    };

    let face_offset = checked_to_usize(
        read_u32(bytes, offset + mesh_offsets::FACE_ARRAY_OFFSET)?,
        "face_offset",
    )?;
    let face_count = checked_to_usize(
        read_u32(bytes, offset + mesh_offsets::FACE_COUNT)?,
        "face_count",
    )?;

    // +0x98: vertex_indices - dead field, skipped.
    // +0xA4: left_over_faces - always empty, skipped.

    let vertex_indices_count_ptr = checked_to_usize(
        read_u32(bytes, offset + mesh_offsets::VERTEX_INDICES_COUNT_ARRAY_PTR)?,
        "vertex_indices_count_ptr",
    )?;
    let vertex_indices_count_count = read_u32(
        bytes,
        offset + mesh_offsets::VERTEX_INDICES_COUNT_ARRAY_COUNT,
    )?;
    let _vertex_indices_count_alloc = read_u32(
        bytes,
        offset + mesh_offsets::VERTEX_INDICES_COUNT_ARRAY_ALLOC,
    )?;

    // +0xBC: mdx_offsets - data derived from faces, only count/ptr used for reading.
    let _mdx_offsets_count = read_u32(bytes, offset + mesh_offsets::MDX_OFFSETS_ARRAY_COUNT)?;
    let _mdx_offsets_alloc = read_u32(bytes, offset + mesh_offsets::MDX_OFFSETS_ARRAY_ALLOC)?;

    let index_buffer_pools_ptr = checked_to_usize(
        read_u32(bytes, offset + mesh_offsets::INDEX_BUFFER_POOLS_ARRAY_PTR)?,
        "index_buffer_pools_ptr",
    )?;
    let index_buffer_pools_count =
        read_u32(bytes, offset + mesh_offsets::INDEX_BUFFER_POOLS_ARRAY_COUNT)?;
    let _index_buffer_pools_alloc =
        read_u32(bytes, offset + mesh_offsets::INDEX_BUFFER_POOLS_ARRAY_ALLOC)?;

    let shared_index_offset = read_i32(bytes, offset + mesh_offsets::SHARED_INDEX_OFFSET)?;
    let shared_index_pool = read_i32(bytes, offset + mesh_offsets::SHARED_INDEX_POOL)?;
    let shared_index_size = read_i32(bytes, offset + mesh_offsets::SHARED_INDEX_SIZE)?;
    let indices_per_face = read_u32(bytes, offset + mesh_offsets::INDICES_PER_FACE)?;

    // Vertex info from Ghidra-verified offsets.
    let vertex_count = usize::from(read_u16(bytes, offset + mesh_offsets::VERTEX_COUNT)?);
    let mdx_data_offset = checked_to_usize(
        read_u32(bytes, offset + k2 + mesh_offsets::MDX_DATA_OFFSET)?,
        "mdx_data_offset",
    )?;
    let vert_array_offset = checked_to_usize(
        read_u32(bytes, offset + k2 + mesh_offsets::VERT_ARRAY_OFFSET)?,
        "vert_array_offset",
    )?;
    let vertex_stride = read_u32(bytes, offset + mesh_offsets::VERTEX_STRUCT_SIZE)?;

    // Render/shadow flags from Ghidra-verified offsets.
    let render = read_u8(bytes, offset + mesh_offsets::RENDER)? != 0;
    let shadow = read_u8(bytes, offset + mesh_offsets::SHADOW)? != 0;

    // Toolset function pointer stubs (extra +0x00, +0x04).
    let fn_ptr_gen_vertices = read_u32(bytes, offset + mesh_offsets::FN_PTR_GEN_VERTICES)?;
    let fn_ptr_remove_temp_array =
        read_u32(bytes, offset + mesh_offsets::FN_PTR_REMOVE_TEMP_ARRAY)?;

    // Bounding box and sphere.
    let bounding_min = [
        read_f32(bytes, offset + mesh_offsets::BOUNDING_MIN)?,
        read_f32(bytes, offset + mesh_offsets::BOUNDING_MIN + 4)?,
        read_f32(bytes, offset + mesh_offsets::BOUNDING_MIN + 8)?,
    ];
    let bounding_max = [
        read_f32(bytes, offset + mesh_offsets::BOUNDING_MAX)?,
        read_f32(bytes, offset + mesh_offsets::BOUNDING_MAX + 4)?,
        read_f32(bytes, offset + mesh_offsets::BOUNDING_MAX + 8)?,
    ];
    let bsphere_radius = read_f32(bytes, offset + mesh_offsets::BSPHERE_RADIUS)?;
    let bsphere_center = [
        read_f32(bytes, offset + mesh_offsets::BSPHERE_CENTER)?,
        read_f32(bytes, offset + mesh_offsets::BSPHERE_CENTER + 4)?,
        read_f32(bytes, offset + mesh_offsets::BSPHERE_CENTER + 8)?,
    ];

    // Colors.
    let diffuse_color = [
        read_f32(bytes, offset + mesh_offsets::DIFFUSE_COLOR)?,
        read_f32(bytes, offset + mesh_offsets::DIFFUSE_COLOR + 4)?,
        read_f32(bytes, offset + mesh_offsets::DIFFUSE_COLOR + 8)?,
    ];
    let ambient_color = [
        read_f32(bytes, offset + mesh_offsets::AMBIENT_COLOR)?,
        read_f32(bytes, offset + mesh_offsets::AMBIENT_COLOR + 4)?,
        read_f32(bytes, offset + mesh_offsets::AMBIENT_COLOR + 8)?,
    ];
    let transparency_hint = read_i32(bytes, offset + mesh_offsets::TRANSPARENCY_HINT)?;

    // Texture names (null-terminated char[32]).
    let texture_0 = read_fixed_string(
        bytes,
        offset + mesh_offsets::TEXTURE_0,
        mesh_offsets::TEXTURE_NAME_SIZE,
    );
    let texture_1 = read_fixed_string(
        bytes,
        offset + mesh_offsets::TEXTURE_1,
        mesh_offsets::TEXTURE_NAME_SIZE,
    );

    // UV animation.
    let animate_uv = read_i32(bytes, offset + mesh_offsets::ANIMATE_UV)?;
    let uv_direction_x = read_f32(bytes, offset + mesh_offsets::UV_DIRECTION_X)?;
    let uv_direction_y = read_f32(bytes, offset + mesh_offsets::UV_DIRECTION_Y)?;
    let uv_jitter = read_f32(bytes, offset + mesh_offsets::UV_JITTER)?;
    let uv_jitter_speed = read_f32(bytes, offset + mesh_offsets::UV_JITTER_SPEED)?;

    // Remaining scalar/boolean fields.
    let texture_channel_count = read_u16(bytes, offset + mesh_offsets::TEXTURE_CHANNEL_COUNT)?;
    let light_mapped = read_u8(bytes, offset + mesh_offsets::LIGHT_MAPPED)? != 0;
    let rotate_texture = read_u8(bytes, offset + mesh_offsets::ROTATE_TEXTURE)? != 0;
    let is_background_geometry =
        read_u8(bytes, offset + mesh_offsets::IS_BACKGROUND_GEOMETRY)? != 0;
    let beaming = read_u8(bytes, offset + mesh_offsets::BEAMING)? != 0;
    let total_surface_area = read_f32(bytes, offset + k2 + mesh_offsets::TOTAL_SURFACE_AREA)?;

    // Read Faces - MaxFace is 32 bytes per entry.
    let mut faces = Vec::with_capacity(face_count);
    if face_count > 0 && face_offset > 0 {
        if face_offset + (face_count * MAX_FACE_SIZE) <= bytes.len() {
            for i in 0..face_count {
                let curr = face_offset + (i * MAX_FACE_SIZE);
                faces.push(MdlFace {
                    plane_normal: [
                        read_f32(bytes, curr)?,
                        read_f32(bytes, curr + 4)?,
                        read_f32(bytes, curr + 8)?,
                    ],
                    plane_distance: read_f32(bytes, curr + 12)?,
                    surface_id: read_u32(bytes, curr + 16)?,
                    adjacent: [
                        read_u16(bytes, curr + 20)?,
                        read_u16(bytes, curr + 22)?,
                        read_u16(bytes, curr + 24)?,
                    ],
                    vertex_indices: [
                        read_u16(bytes, curr + 26)?,
                        read_u16(bytes, curr + 28)?,
                        read_u16(bytes, curr + 30)?,
                    ],
                });
            }
        } else {
            crate::trace_warn!(
                face_offset,
                face_count,
                bytes_len = bytes.len(),
                "face array extends beyond buffer, skipping faces"
            );
        }
    }

    // --- TriMesh internal CExoArrayList typed extraction ---
    // These 5 fields are now stored as typed values rather than raw blobs.
    // See `docs/src/formats/models/mesh_derived_fields.md` for full documentation.

    // +0x98: Dead field (always zeros in KotOR). Skipped - writer emits zeros.

    // +0xC8: Inverted counter (single u32 data value).
    let inverted_counter = if index_buffer_pools_count > 0 && index_buffer_pools_ptr > 0 {
        if index_buffer_pools_ptr + 4 <= bytes.len() {
            read_u32(bytes, index_buffer_pools_ptr)?
        } else {
            0
        }
    } else {
        0
    };

    // +0xB0: Detect embedded-position variant (count==1 with positions at ptr+4).
    let has_embedded_positions = vertex_indices_count_count == 1
        && vertex_indices_count_ptr > 0
        && vert_array_offset == vertex_indices_count_ptr.saturating_add(4);

    // Read per-attribute byte offsets from mesh header (+0x104..+0x120).
    // Each is an i32; value -1 (0xFFFFFFFF) means not present.
    let pos_off = read_i32(bytes, offset + mesh_offsets::MDX_POSITION_OFFSET)?;
    let norm_off = read_i32(bytes, offset + mesh_offsets::MDX_NORMAL_OFFSET)?;
    let color_off = read_i32(bytes, offset + mesh_offsets::MDX_COLOR_OFFSET)?;
    let uv1_off = read_i32(bytes, offset + mesh_offsets::MDX_UV1_OFFSET)?;
    let uv2_off = read_i32(bytes, offset + mesh_offsets::MDX_UV2_OFFSET)?;
    let uv3_off = read_i32(bytes, offset + mesh_offsets::MDX_UV3_OFFSET)?;
    let uv4_off = read_i32(bytes, offset + mesh_offsets::MDX_UV4_OFFSET)?;
    let tangent_off = read_i32(bytes, offset + mesh_offsets::MDX_TANGENT_SPACE_OFFSET)?;

    let info = MdxMeshInfo {
        stride: vertex_stride,
        vertex_count,
        mdx_data_offset,
        vert_array_offset,
        pos_off,
        norm_off,
        color_off,
        uv1_off,
        uv2_off,
        uv3_off,
        uv4_off,
        tangent_off,
        bone_weights_off: -1,
        bone_indices_off: -1,
    };

    let mesh = MdlMesh {
        fn_ptr_gen_vertices,
        fn_ptr_remove_temp_array,
        bounding_min,
        bounding_max,
        bsphere_radius,
        bsphere_center,
        diffuse_color,
        ambient_color,
        transparency_hint,
        texture_0,
        texture_1,
        animate_uv,
        uv_direction_x,
        uv_direction_y,
        uv_jitter,
        uv_jitter_speed,
        texture_channel_count,
        light_mapped,
        rotate_texture,
        is_background_geometry,
        beaming,
        total_surface_area,
        positions: Vec::new(),
        normals: Vec::new(),
        vertex_colors: Vec::new(),
        uv1: Vec::new(),
        uv2: Vec::new(),
        uv3: Vec::new(),
        uv4: Vec::new(),
        tangent_space: Vec::new(),
        faces,
        inverted_counter,
        has_embedded_positions,
        shared_index_offset,
        shared_index_pool,
        shared_index_size,
        indices_per_face,
        vertex_count: u16::try_from(vertex_count)
            .map_err(|_| MdlError::ValueOverflow("vertex_count"))?,
        render,
        shadow,
        tsl: tsl_fields,
    };

    Ok((mesh, info))
}

// ---------------------------------------------------------------------------
// Pass 2: MDX vertex data population (per-mesh seeking via mdx_data_offset)
// ---------------------------------------------------------------------------

/// Reads MDX vertex data using each mesh's `mdx_data_offset` (+0x144) to seek
/// directly to the correct position in the MDX buffer.
///
/// This matches how the engine and community tools (kotorblender, mdledit)
/// consume MDX data -- each mesh's header stores the byte offset where its
/// interleaved vertex data begins in the MDX file, so the reader does not
/// need to assume any particular mesh ordering within the MDX buffer.
fn populate_mdx_data(
    root: &mut MdlNode,
    mdx: &[u8],
    infos: &[MdxMeshInfo],
) -> Result<(), MdlError> {
    if infos.is_empty() {
        return Ok(());
    }

    // Read vertex data for each mesh by seeking to its mdx_data_offset.
    let mut results: Vec<MdxVertexData> =
        (0..infos.len()).map(|_| MdxVertexData::default()).collect();

    for (idx, info) in infos.iter().enumerate() {
        let data = read_mdx_vertices(mdx, info)?;
        results[idx] = data;
    }

    // Apply vertex data to the tree in DFS order.
    let mut counter = 0;
    apply_vertex_data(root, &mut results, &mut counter);

    Ok(())
}

/// Reads all vertex attributes for a single mesh from the MDX buffer,
/// seeking directly to the mesh's `mdx_data_offset` position.
fn read_mdx_vertices(mdx: &[u8], info: &MdxMeshInfo) -> Result<MdxVertexData, MdlError> {
    let mut data = MdxVertexData::default();

    if info.vertex_count == 0 || info.stride == 0 {
        return Ok(data);
    }

    let stride = checked_to_usize(info.stride, "vertex_stride")?;
    let mdx_base = info.mdx_data_offset;

    for i in 0..info.vertex_count {
        let base = mdx_base + (i * stride);
        if base + stride > mdx.len() {
            crate::trace_warn!(
                vertex_index = i,
                base,
                stride,
                mdx_len = mdx.len(),
                mdx_cursor = mdx_base,
                "MDX vertex data truncated"
            );
            break;
        }

        // Position (3xf32 = 12 bytes)
        if let Ok(off) = usize::try_from(info.pos_off) {
            let o = base + off;
            data.positions.push([
                read_f32(mdx, o)?,
                read_f32(mdx, o + 4)?,
                read_f32(mdx, o + 8)?,
            ]);
        }

        // Normal (3xf32 = 12 bytes)
        if let Ok(off) = usize::try_from(info.norm_off) {
            let o = base + off;
            data.normals.push([
                read_f32(mdx, o)?,
                read_f32(mdx, o + 4)?,
                read_f32(mdx, o + 8)?,
            ]);
        }

        // Vertex color (4xu8 = 4 bytes)
        if let Ok(off) = usize::try_from(info.color_off) {
            let o = base + off;
            data.vertex_colors.push([
                read_u8(mdx, o)?,
                read_u8(mdx, o + 1)?,
                read_u8(mdx, o + 2)?,
                read_u8(mdx, o + 3)?,
            ]);
        }

        // UV1 (2xf32 = 8 bytes)
        if let Ok(off) = usize::try_from(info.uv1_off) {
            let o = base + off;
            data.uv1.push([read_f32(mdx, o)?, read_f32(mdx, o + 4)?]);
        }

        // UV2 (2xf32 = 8 bytes)
        if let Ok(off) = usize::try_from(info.uv2_off) {
            let o = base + off;
            data.uv2.push([read_f32(mdx, o)?, read_f32(mdx, o + 4)?]);
        }

        // UV3 (2xf32 = 8 bytes)
        if let Ok(off) = usize::try_from(info.uv3_off) {
            let o = base + off;
            data.uv3.push([read_f32(mdx, o)?, read_f32(mdx, o + 4)?]);
        }

        // UV4 (2xf32 = 8 bytes)
        if let Ok(off) = usize::try_from(info.uv4_off) {
            let o = base + off;
            data.uv4.push([read_f32(mdx, o)?, read_f32(mdx, o + 4)?]);
        }

        // Tangent space (3x3xf32 = 36 bytes)
        if let Ok(off) = usize::try_from(info.tangent_off) {
            let o = base + off;
            data.tangent_space.push([
                [
                    read_f32(mdx, o)?,
                    read_f32(mdx, o + 4)?,
                    read_f32(mdx, o + 8)?,
                ],
                [
                    read_f32(mdx, o + 12)?,
                    read_f32(mdx, o + 16)?,
                    read_f32(mdx, o + 20)?,
                ],
                [
                    read_f32(mdx, o + 24)?,
                    read_f32(mdx, o + 28)?,
                    read_f32(mdx, o + 32)?,
                ],
            ]);
        }

        // Bone weights (4xf32 = 16 bytes)
        if let Ok(off) = usize::try_from(info.bone_weights_off) {
            let o = base + off;
            data.bone_weights.push([
                read_f32(mdx, o)?,
                read_f32(mdx, o + 4)?,
                read_f32(mdx, o + 8)?,
                read_f32(mdx, o + 12)?,
            ]);
        }

        // Bone indices (4xf32 = 16 bytes)
        if let Ok(off) = usize::try_from(info.bone_indices_off) {
            let o = base + off;
            data.bone_indices.push([
                read_f32(mdx, o)?,
                read_f32(mdx, o + 4)?,
                read_f32(mdx, o + 8)?,
                read_f32(mdx, o + 12)?,
            ]);
        }
    }

    // Clamp vertex_count to actual vertex data length. Vanilla item models
    // may declare more vertices than the MDX buffer contains (shared buffer
    // truncation). The typed vertex_count must match the actual data arrays
    // for roundtrip fidelity - the writer uses vertex_count to size the MDX
    // output region, and over-declaring causes cross-contamination between
    // meshes on re-read.
    let actual_vertex_count = data
        .positions
        .len()
        .max(data.normals.len())
        .max(data.vertex_colors.len())
        .max(data.uv1.len())
        .max(data.uv2.len())
        .max(data.uv3.len())
        .max(data.uv4.len())
        .max(data.tangent_space.len());

    if actual_vertex_count > 0 && actual_vertex_count < info.vertex_count {
        data.clamped_vertex_count = Some(
            u16::try_from(actual_vertex_count)
                .map_err(|_| MdlError::ValueOverflow("actual_vertex_count"))?,
        );
    }

    Ok(data)
}

/// Reads position-only vertex data from the MDL content blob (no-MDX fallback).
///
/// Each mesh's `vert_array_offset` (+0x148) is a content-relative offset into
/// the MDL blob pointing to position-only data (12 bytes per vertex = 3x f32).
/// This path is used when no MDX file is available. Ordering doesn't matter
/// here since each mesh reads from its own independent content offset.
fn populate_content_positions(
    root: &mut MdlNode,
    bytes: &[u8],
    infos: &[MdxMeshInfo],
) -> Result<(), MdlError> {
    if infos.is_empty() {
        return Ok(());
    }

    // Read position-only data for each mesh from its stored MDL content offset.
    let mut results: Vec<MdxVertexData> =
        (0..infos.len()).map(|_| MdxVertexData::default()).collect();

    for (idx, info) in infos.iter().enumerate() {
        if info.vertex_count == 0 || info.vert_array_offset == 0 {
            continue;
        }

        let pos_end = info.vert_array_offset + (info.vertex_count * 12);
        if pos_end <= bytes.len() {
            let mut positions = Vec::with_capacity(info.vertex_count);
            for i in 0..info.vertex_count {
                let o = info.vert_array_offset + (i * 12);
                positions.push([
                    read_f32(bytes, o)?,
                    read_f32(bytes, o + 4)?,
                    read_f32(bytes, o + 8)?,
                ]);
            }
            results[idx].positions = positions;
        } else {
            crate::trace_warn!(
                vert_array_offset = info.vert_array_offset,
                vertex_count = info.vertex_count,
                pos_end,
                bytes_len = bytes.len(),
                "MDL content position data extends beyond buffer"
            );
        }
    }

    // Apply to tree in DFS order.
    let mut counter = 0;
    apply_vertex_data(root, &mut results, &mut counter);

    Ok(())
}

/// Reads content positions for mesh nodes that the MDX pass skipped.
///
/// After `populate_mdx_data`, mesh nodes with stride=0 (e.g., saber nodes)
/// have empty position arrays because the MDX path returns no data for them.
/// However, these nodes may still have valid embedded positions at their
/// `vert_array_offset` (+0x148). This function fills in positions only for
/// nodes where the MDX pass left positions empty.
fn populate_content_positions_fallback(
    root: &mut MdlNode,
    bytes: &[u8],
    infos: &[MdxMeshInfo],
) -> Result<(), MdlError> {
    if infos.is_empty() {
        return Ok(());
    }

    let mut counter = 0;
    fill_missing_positions(root, bytes, infos, &mut counter)?;
    Ok(())
}

/// Recursive helper: walks DFS and reads content positions for mesh nodes
/// that still have empty positions but a valid vert_array_offset.
fn fill_missing_positions(
    node: &mut MdlNode,
    bytes: &[u8],
    infos: &[MdxMeshInfo],
    counter: &mut usize,
) -> Result<(), MdlError> {
    if node.node_data.mesh().is_some() {
        let idx = *counter;
        *counter += 1;

        if let (Some(info), Some(mesh)) = (infos.get(idx), node.node_data.mesh_mut()) {
            if mesh.positions.is_empty() && info.vertex_count > 0 && info.vert_array_offset > 0 {
                let pos_end = info.vert_array_offset + (info.vertex_count * 12);
                if pos_end <= bytes.len() {
                    let mut positions = Vec::with_capacity(info.vertex_count);
                    for i in 0..info.vertex_count {
                        let o = info.vert_array_offset + (i * 12);
                        positions.push([
                            read_f32(bytes, o)?,
                            read_f32(bytes, o + 4)?,
                            read_f32(bytes, o + 8)?,
                        ]);
                    }
                    mesh.positions = positions;
                } else {
                    crate::trace_warn!(
                        vert_array_offset = info.vert_array_offset,
                        vertex_count = info.vertex_count,
                        pos_end,
                        bytes_len = bytes.len(),
                        "fallback content position data extends beyond buffer"
                    );
                }
            }
        }
    }

    for child in &mut node.children {
        fill_missing_positions(child, bytes, infos, counter)?;
    }

    Ok(())
}

/// Walks the node tree in DFS order, applying vertex data from `results`
/// to each mesh node. The results are indexed by DFS mesh order (matching
/// the order meshes were encountered during Pass 1).
fn apply_vertex_data(node: &mut MdlNode, results: &mut Vec<MdxVertexData>, counter: &mut usize) {
    if node.node_data.mesh().is_some() {
        let idx = *counter;
        *counter += 1;

        if idx < results.len() {
            let mut data = std::mem::take(&mut results[idx]);
            let bone_weights = std::mem::take(&mut data.bone_weights);
            let bone_indices = std::mem::take(&mut data.bone_indices);

            if let Some(mesh) = node.node_data.mesh_mut() {
                mesh.positions = data.positions;
                mesh.normals = data.normals;
                mesh.vertex_colors = data.vertex_colors;
                mesh.uv1 = data.uv1;
                mesh.uv2 = data.uv2;
                mesh.uv3 = data.uv3;
                mesh.uv4 = data.uv4;
                mesh.tangent_space = data.tangent_space;
                if let Some(clamped) = data.clamped_vertex_count {
                    mesh.vertex_count = clamped;
                }
            }

            // Move bone weight/index data to the skin wrapper (only skins
            // carry these fields).
            if let MdlNodeData::Skin(skin) = &mut node.node_data {
                skin.bone_weights = bone_weights;
                skin.bone_indices = bone_indices;
            }
        }
    }

    for child in &mut node.children {
        apply_vertex_data(child, results, counter);
    }
}

// ---------------------------------------------------------------------------
// Non-mesh node type readers (unchanged)
// ---------------------------------------------------------------------------

/// Reads a fixed-size null-terminated string from the byte buffer.
fn read_fixed_string(bytes: &[u8], offset: usize, max_len: usize) -> String {
    rakata_formats::binary::read_fixed_c_string(bytes, offset, max_len)
}

/// Reads a variable-length null-terminated string from the byte buffer.
fn read_cstring(bytes: &[u8], offset: usize) -> String {
    rakata_formats::binary::read_c_string(bytes, offset)
}

/// Reads a CExoArrayList (ptr/count pair) with bounds checking and OOB warning.
///
/// Extracts the content-relative pointer and element count from the given field
/// offsets, performs a bounds check, and delegates to `reader(bytes, ptr, count)`.
/// Returns an empty vec if count is zero, ptr is null, or data is out of bounds.
fn read_cexo_array<T>(
    bytes: &[u8],
    offset: usize,
    ptr_field: usize,
    count_field: usize,
    element_size: usize,
    label: &'static str,
    reader: impl FnOnce(&[u8], usize, usize) -> Result<Vec<T>, MdlError>,
) -> Result<Vec<T>, MdlError> {
    let ptr = checked_to_usize(read_u32(bytes, offset + ptr_field)?, label)?;
    let count = checked_to_usize(read_u32(bytes, offset + count_field)?, label)?;

    if count == 0 || ptr == 0 {
        return Ok(Vec::new());
    }

    let byte_size = count * element_size;
    if ptr + byte_size > bytes.len() {
        crate::trace_warn!(
            ptr,
            count,
            bytes_len = bytes.len(),
            label,
            "CExoArrayList extends beyond buffer"
        );
        return Ok(Vec::new());
    }

    reader(bytes, ptr, count)
}

/// Reads `count` sequential `f32` values starting at `ptr`.
fn read_f32_array(bytes: &[u8], ptr: usize, count: usize) -> Result<Vec<f32>, MdlError> {
    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        result.push(read_f32(bytes, ptr + i * 4)?);
    }
    Ok(result)
}

/// Reads `count` sequential `[f32; 3]` vectors (12 bytes each) starting at `ptr`.
fn read_vec3_array(bytes: &[u8], ptr: usize, count: usize) -> Result<Vec<[f32; 3]>, MdlError> {
    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let base = ptr + i * 12;
        result.push([
            read_f32(bytes, base)?,
            read_f32(bytes, base + 4)?,
            read_f32(bytes, base + 8)?,
        ]);
    }
    Ok(result)
}

/// Reads `count` sequential `[f32; 4]` quaternions (16 bytes each) starting at `ptr`.
fn read_quat_array(bytes: &[u8], ptr: usize, count: usize) -> Result<Vec<[f32; 4]>, MdlError> {
    let mut result = Vec::with_capacity(count);
    for i in 0..count {
        let base = ptr + i * 16;
        result.push([
            read_f32(bytes, base)?,
            read_f32(bytes, base + 4)?,
            read_f32(bytes, base + 8)?,
            read_f32(bytes, base + 12)?,
        ]);
    }
    Ok(result)
}

/// Reads a Reference node header (36 extra bytes after the base node header).
///
/// Layout: `ref_model` (char[32]) + `reattachable` (i32).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_reference_header(bytes: &[u8], offset: usize) -> Result<MdlReference, MdlError> {
    check_slice_in_bounds(bytes, offset, REFERENCE_EXTRA_SIZE, "reference_header")?;

    let ref_model = read_fixed_string(bytes, offset, 32);
    let reattachable = read_i32(bytes, offset + 0x20)?;

    Ok(MdlReference {
        ref_model,
        reattachable,
    })
}

/// Reads a Light node header (92 extra bytes after the base node header).
///
/// Scalar fields are parsed into typed fields. Array headers (5 x 12 bytes)
/// are preserved as raw bytes since they contain file-relative pointers.
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
/// Reads Light extension fields (92 extra bytes after the base node header).
///
/// Parses scalar fields and follows CExoArrayList pointers for flare data:
/// sizes, positions, color shifts, and texture names.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_light_header(bytes: &[u8], offset: usize) -> Result<MdlLight, MdlError> {
    check_slice_in_bounds(bytes, offset, LIGHT_EXTRA_SIZE, "light_header")?;

    let flare_radius = read_f32(bytes, offset + light_offsets::FLARE_RADIUS)?;

    // Texture SafePointers (runtime-only, 3xu32 at +0x04).
    let sp_base = offset + light_offsets::TEXTURE_SAFE_PTRS_PTR;
    let texture_safe_ptrs = [
        read_u32(bytes, sp_base)?,
        read_u32(bytes, sp_base + 4)?,
        read_u32(bytes, sp_base + 8)?,
    ];

    // Flare sizes: CExoArrayList<float> at +0x10
    let flare_sizes = read_cexo_array(
        bytes,
        offset,
        light_offsets::FLARE_SIZES_PTR,
        light_offsets::FLARE_SIZES_COUNT,
        4,
        "flare_sizes",
        read_f32_array,
    )?;

    // Flare positions: CExoArrayList<float> at +0x1C
    let flare_positions = read_cexo_array(
        bytes,
        offset,
        light_offsets::FLARE_POSITIONS_PTR,
        light_offsets::FLARE_POSITIONS_COUNT,
        4,
        "flare_positions",
        read_f32_array,
    )?;

    // Flare color shifts: CExoArrayList<Vector> at +0x28
    let flare_color_shifts = read_cexo_array(
        bytes,
        offset,
        light_offsets::FLARE_COLOR_SHIFTS_PTR,
        light_offsets::FLARE_COLOR_SHIFTS_COUNT,
        12,
        "flare_color_shifts",
        read_vec3_array,
    )?;

    // Flare texture names: CExoArrayList<char*> at +0x34
    // Each entry is a u32 content-relative offset to a null-terminated string.
    let flare_texture_names = read_cexo_array(
        bytes,
        offset,
        light_offsets::FLARE_TEX_NAMES_PTR,
        light_offsets::FLARE_TEX_NAMES_COUNT,
        4,
        "flare_tex_names",
        |b, ptr, count| {
            let mut names = Vec::with_capacity(count);
            for i in 0..count {
                let str_offset =
                    checked_to_usize(read_u32(b, ptr + i * 4)?, "flare_tex_name_str_ptr")?;
                if str_offset > 0 && str_offset < b.len() {
                    names.push(read_cstring(b, str_offset));
                } else {
                    names.push(String::new());
                }
            }
            Ok(names)
        },
    )?;

    let priority = read_i32(bytes, offset + light_offsets::PRIORITY)?;
    let num_dynamic_types = read_i32(bytes, offset + light_offsets::NUM_DYNAMIC_TYPES)?;
    let affectdynamic = read_i32(bytes, offset + light_offsets::AFFECTDYNAMIC)?;
    let shadow = read_i32(bytes, offset + light_offsets::SHADOW)?;
    let ambientonly = read_i32(bytes, offset + light_offsets::AMBIENTONLY)?;
    let generateflare = read_i32(bytes, offset + light_offsets::GENERATEFLARE)?;
    let fading_light = read_i32(bytes, offset + light_offsets::FADING_LIGHT)?;

    Ok(MdlLight {
        flare_radius,
        texture_safe_ptrs,
        flare_sizes,
        flare_positions,
        flare_color_shifts,
        flare_texture_names,
        priority,
        num_dynamic_types,
        affectdynamic,
        shadow,
        ambientonly,
        generateflare,
        fading_light,
    })
}

/// Reads an Emitter node header (224 extra bytes after the base node header).
///
/// All fields are inline fixed-size data - no pointer relocation needed.
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_emitter_header(bytes: &[u8], offset: usize) -> Result<super::types::MdlEmitter, MdlError> {
    check_slice_in_bounds(bytes, offset, EMITTER_EXTRA_SIZE, "emitter_header")?;

    let deadspace = read_f32(bytes, offset)?;
    let blast_radius = read_f32(bytes, offset + 0x04)?;
    let blast_length = read_f32(bytes, offset + 0x08)?;
    let num_branches = read_i32(bytes, offset + 0x0C)?;
    let control_pt_smoothing = read_i32(bytes, offset + 0x10)?;
    let x_grid = read_i32(bytes, offset + 0x14)?;
    let y_grid = read_i32(bytes, offset + 0x18)?;
    let spawn_type = read_i32(bytes, offset + 0x1C)?;

    let update = read_fixed_string(bytes, offset + 0x20, 32);
    let render = read_fixed_string(bytes, offset + 0x40, 32);
    let blend = read_fixed_string(bytes, offset + 0x60, 32);
    let texture = read_fixed_string(bytes, offset + 0x80, 32);
    let chunk_name = read_fixed_string(bytes, offset + 0xA0, 16);

    let two_sided_tex = read_i32(bytes, offset + 0xB0)?;
    let loop_emitter = read_i32(bytes, offset + 0xB4)?;
    let render_order = read_u16(bytes, offset + 0xB8)?;
    let frame_blending = read_u8(bytes, offset + 0xBA)? != 0;
    let depth_texture_name = read_fixed_string(bytes, offset + 0xBB, 16);

    // +0xCB..+0xE0: 21 bytes reserved/padding - read verbatim
    let mut reserved = [0u8; 21];
    reserved.copy_from_slice(&bytes[offset + 0xCB..offset + 0xE0]);

    Ok(super::types::MdlEmitter {
        deadspace,
        blast_radius,
        blast_length,
        num_branches,
        control_pt_smoothing,
        x_grid,
        y_grid,
        spawn_type,
        update,
        render,
        blend,
        texture,
        chunk_name,
        two_sided_tex,
        loop_emitter,
        render_order,
        frame_blending,
        depth_texture_name,
        reserved,
    })
}

/// Reads DanglyMesh extension fields (28 extra bytes after the TriMesh header).
///
/// Parses the constraint array (per-vertex floats), three inline physics
/// parameters, and the per-vertex dangly position array.
///
/// The data pointer at +0x18 references `vertex_count` vec3 positions in the
/// MDL content blob. At runtime, `PartDanglyMesh` (`0x00447980`) copies these
/// into a GL vertex pool for the dangly physics simulation.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_dangly_extra(bytes: &[u8], offset: usize, mesh: MdlMesh) -> Result<MdlDangly, MdlError> {
    check_slice_in_bounds(bytes, offset, DANGLY_EXTRA_SIZE, "dangly_extra")?;

    // CExoArrayList<float> at +0x00: per-vertex constraint weights.
    let constraints = read_cexo_array(
        bytes,
        offset,
        dangly_offsets::CONSTRAINTS_PTR,
        dangly_offsets::CONSTRAINTS_COUNT,
        4,
        "dangly_constraints",
        read_f32_array,
    )?;

    let displacement = read_f32(bytes, offset + dangly_offsets::DISPLACEMENT)?;
    let tightness = read_f32(bytes, offset + dangly_offsets::TIGHTNESS)?;
    let period = read_f32(bytes, offset + dangly_offsets::PERIOD)?;

    // Conditional data pointer at +0x18: vertex_count x vec3 positions.
    // Relocated against MDL content base when vertex_count > 0 (ResetDangly).
    let dangly_verts_ptr = checked_to_usize(
        read_u32(bytes, offset + dangly_offsets::DATA_PTR)?,
        "dangly_verts_ptr",
    )?;

    // Read per-vertex dangly positions (vertex_count x vec3, 12 bytes each).
    let vert_count = usize::from(mesh.vertex_count);
    let mut dangly_vertices = Vec::with_capacity(vert_count);
    if vert_count > 0 && dangly_verts_ptr > 0 {
        let required = vert_count * 12;
        if dangly_verts_ptr + required <= bytes.len() {
            dangly_vertices = read_vec3_array(bytes, dangly_verts_ptr, vert_count)?;
        } else {
            crate::trace_warn!(
                dangly_verts_ptr,
                vert_count,
                bytes_len = bytes.len(),
                "dangly vertices array extends beyond buffer"
            );
        }
    }

    Ok(MdlDangly {
        mesh,
        constraints,
        displacement,
        tightness,
        period,
        dangly_vertices,
    })
}

/// Reads Skin extension fields (100 extra bytes after the TriMesh header).
///
/// Parses the weights CExoArrayList, MDX bone weight/index offsets, bonemap,
/// three CExoArrayList arrays (qbone, tbone, bone_constant_indices), and the
/// fixed-size bone_node_numbers array.
///
/// All fields are fully typed - no raw blob is preserved.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_skin_extra(bytes: &[u8], offset: usize, mesh: MdlMesh) -> Result<MdlSkin, MdlError> {
    check_slice_in_bounds(bytes, offset, SKIN_EXTRA_SIZE, "skin_extra")?;

    // Weights CExoArrayList header at +0x00: ptr, count, alloc (12 bytes).
    // Always zeros in vanilla binary files - engine uses MDX bone data instead.
    // SkinVertexWeight (52 bytes/element) is only populated by the ASCII parser.
    // Weights CExoArrayList at +0x00: always zeros in binary - writer emits zeros.

    // MDX per-vertex bone weight/index offsets at +0x0C/+0x10.
    let mdx_bone_weights_offset = read_i32(bytes, offset + skin_offsets::MDX_BONE_WEIGHTS_OFFSET)?;
    let mdx_bone_indices_offset = read_i32(bytes, offset + skin_offsets::MDX_BONE_INDICES_OFFSET)?;

    // Bonemap pointer + count at +0x14/+0x18.
    let bonemap_ptr = checked_to_usize(
        read_u32(bytes, offset + skin_offsets::BONEMAP_PTR)?,
        "skin_bonemap_ptr",
    )?;
    let bonemap_count = read_u32(bytes, offset + skin_offsets::BONEMAP_COUNT)?;

    let mut bonemap = Vec::new();
    if bonemap_count > 0 && bonemap_ptr > 0 {
        let entry_count = checked_to_usize(bonemap_count, "bonemap_count")?;
        let byte_len = entry_count.checked_mul(4).unwrap_or(0);
        if byte_len > 0 && bonemap_ptr + byte_len <= bytes.len() {
            bonemap.reserve(entry_count);
            for i in 0..entry_count {
                bonemap.push(read_u32(bytes, bonemap_ptr + i * 4)?);
            }
        } else {
            crate::trace_warn!(
                bonemap_ptr,
                bonemap_count,
                bytes_len = bytes.len(),
                "skin bonemap extends beyond buffer"
            );
        }
    }

    // CExoArrayList<Quaternion> at +0x1C: inverse bind rotations
    let qbone_ref_inv = read_cexo_array(
        bytes,
        offset,
        skin_offsets::QBONE_REF_INV_PTR,
        skin_offsets::QBONE_REF_INV_COUNT,
        16,
        "skin_qbone_ref_inv",
        read_quat_array,
    )?;

    // CExoArrayList<Vector> at +0x28: inverse bind translations
    let tbone_ref_inv = read_cexo_array(
        bytes,
        offset,
        skin_offsets::TBONE_REF_INV_PTR,
        skin_offsets::TBONE_REF_INV_COUNT,
        12,
        "skin_tbone_ref_inv",
        read_vec3_array,
    )?;

    // CExoArrayList<int> at +0x34: bone constant indices
    let bone_constant_indices = read_cexo_array(
        bytes,
        offset,
        skin_offsets::BONE_CONSTANT_INDICES_PTR,
        skin_offsets::BONE_CONSTANT_INDICES_COUNT,
        4,
        "skin_bone_constant_indices",
        |b, ptr, count| {
            let mut indices = Vec::with_capacity(count);
            for i in 0..count {
                indices.push(read_i32(b, ptr + (i * 4))?);
            }
            Ok(indices)
        },
    )?;

    // bone_node_numbers: 16 x u16 at +0x40.
    let mut bone_node_numbers = [0u16; 16];
    let bnn_offset = offset + skin_offsets::BONE_NODE_NUMBERS;
    for (i, slot) in bone_node_numbers.iter_mut().enumerate() {
        *slot = read_u16(bytes, bnn_offset + i * 2)?;
    }

    // +0x60..+0x63: Padding (leaked runtime pointers in ~74 vanilla models).
    // Writer emits zeros - this is garbage data the engine doesn't read.

    Ok(MdlSkin {
        mesh,
        mdx_bone_weights_offset,
        mdx_bone_indices_offset,
        bone_weights: Vec::new(),
        bone_indices: Vec::new(),
        bonemap,
        qbone_ref_inv,
        tbone_ref_inv,
        bone_constant_indices,
        bone_node_numbers,
    })
}

/// Reads AnimMesh extension fields (56 extra bytes after the TriMesh header).
///
/// Parses one inline scalar (`sample_period`), two CExoArrayList arrays,
/// and six runtime-only fields:
/// - `anim_verts`: animated vertex positions (Vector = 3 x f32 each)
/// - `anim_t_verts`: animated texture coordinates (Vector = 3 x f32 each)
/// - Runtime fields at +0x1C..+0x37: always zero in authored files, preserved
///   for roundtrip fidelity.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_anim_mesh_extra(
    bytes: &[u8],
    offset: usize,
    mesh: MdlMesh,
) -> Result<MdlAnimMesh, MdlError> {
    check_slice_in_bounds(bytes, offset, ANIM_MESH_EXTRA_SIZE, "anim_mesh_extra")?;

    let sample_period = read_f32(bytes, offset + anim_mesh_offsets::SAMPLE_PERIOD)?;

    // CExoArrayList<Vector> at +0x04: animated vertex positions
    let anim_verts = read_cexo_array(
        bytes,
        offset,
        anim_mesh_offsets::ANIM_VERTS_PTR,
        anim_mesh_offsets::ANIM_VERTS_COUNT,
        12,
        "anim_verts",
        read_vec3_array,
    )?;

    // CExoArrayList<Vector> at +0x10: animated texture coordinates
    let anim_t_verts = read_cexo_array(
        bytes,
        offset,
        anim_mesh_offsets::ANIM_T_VERTS_PTR,
        anim_mesh_offsets::ANIM_T_VERTS_COUNT,
        12,
        "anim_t_verts",
        read_vec3_array,
    )?;

    // Runtime-only fields at +0x1C..+0x37 (no ASCII parser names).
    // Always zero in authored files; preserved for roundtrip fidelity.
    let data_ptr_1 = read_u32(bytes, offset + anim_mesh_offsets::DATA_PTR_1)?;
    let data_count_1 = read_u32(bytes, offset + anim_mesh_offsets::DATA_COUNT_1)?;
    let padding_24 = read_u32(bytes, offset + anim_mesh_offsets::PADDING_24)?;
    let anim_vertices_ptr = read_u32(bytes, offset + anim_mesh_offsets::ANIM_VERTICES_PTR)?;
    let anim_tex_vertices_ptr = read_u32(bytes, offset + anim_mesh_offsets::ANIM_TEX_VERTICES_PTR)?;
    let anim_vertices_count_val = read_u32(bytes, offset + anim_mesh_offsets::ANIM_VERTICES_COUNT)?;
    let anim_tex_vertices_count =
        read_u32(bytes, offset + anim_mesh_offsets::ANIM_TEX_VERTICES_COUNT)?;

    Ok(MdlAnimMesh {
        mesh,
        sample_period,
        anim_verts,
        anim_t_verts,
        data_ptr_1,
        data_count_1,
        padding_24,
        anim_vertices_ptr,
        anim_tex_vertices_ptr,
        anim_vertices_count: anim_vertices_count_val,
        anim_tex_vertices_count,
    })
}

/// Reads AABB extension fields (4 extra bytes after the TriMesh header).
///
/// The 4-byte extra header contains a single pointer to the AABB binary
/// search tree root. The tree is parsed recursively by following child
/// pointers - each node is 40 bytes containing bounding box, child
/// pointers, face index, and split direction flags.
///
/// See `docs/src/internals/mdl_deep_dive.md` "AABB walkmesh".
fn read_aabb_extra(
    bytes: &[u8],
    offset: usize,
    _node_data_end_hint: usize,
    mesh: MdlMesh,
) -> Result<MdlAabb, MdlError> {
    check_slice_in_bounds(bytes, offset, AABB_EXTRA_SIZE, "aabb_extra")?;

    let aabb_tree_ptr = checked_to_usize(
        read_u32(bytes, offset + aabb_offsets::TREE_PTR)?,
        "aabb_tree_ptr",
    )?;

    let aabb_tree = if aabb_tree_ptr > 0 {
        Some(Box::new(read_aabb_tree(bytes, aabb_tree_ptr)?))
    } else {
        None
    };

    Ok(MdlAabb { mesh, aabb_tree })
}

/// Recursively reads an AABB binary search tree node and its children.
///
/// Each node is 40 bytes on disk. Child pointers are content-blob-relative
/// offsets (0 = no child / leaf). The tree is followed depth-first.
///
/// See `docs/src/internals/mdl_deep_dive.md` "AABB walkmesh".
fn read_aabb_tree(bytes: &[u8], ptr: usize) -> Result<AabbNode, MdlError> {
    const AABB_NODE_SIZE: usize = 40;

    check_slice_in_bounds(bytes, ptr, AABB_NODE_SIZE, "aabb_node")?;

    let box_min = [
        read_f32(bytes, ptr)?,
        read_f32(bytes, ptr + 4)?,
        read_f32(bytes, ptr + 8)?,
    ];
    let box_max = [
        read_f32(bytes, ptr + 12)?,
        read_f32(bytes, ptr + 16)?,
        read_f32(bytes, ptr + 20)?,
    ];
    // Note: right_child at +0x18, left_child at +0x1C (Ghidra struct order).
    let right_ptr = checked_to_usize(read_u32(bytes, ptr + 24)?, "aabb_right_child")?;
    let left_ptr = checked_to_usize(read_u32(bytes, ptr + 28)?, "aabb_left_child")?;
    let face_index = read_i32(bytes, ptr + 32)?;
    let split_direction_flags = read_u32(bytes, ptr + 36)?;

    let left = if left_ptr > 0 {
        Some(Box::new(read_aabb_tree(bytes, left_ptr)?))
    } else {
        None
    };
    let right = if right_ptr > 0 {
        Some(Box::new(read_aabb_tree(bytes, right_ptr)?))
    } else {
        None
    };

    Ok(AabbNode {
        box_min,
        box_max,
        face_index,
        split_direction_flags,
        left,
        right,
    })
}

/// Reads Saber extension fields (20 extra bytes after the TriMesh header).
///
/// The 20-byte extra header contains 3 relocated data pointers and 2 runtime
/// GL pool IDs. All fields are preserved as raw bytes - the data pointers
/// contain file-relative offsets and the GL pool IDs are runtime-only values.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
/// Reads Saber extension fields (20 extra bytes after the TriMesh header).
///
/// Structurally parses the three saber vertex arrays (positions, UVs, normals)
/// at known fixed sizes (176 vertices each), plus two runtime GL pool IDs.
///
/// kotorblender names these `off_saber_verts`, `off_saber_uv` and
/// `off_saber_normals`. Noted for cross-reference; the identifiers below are
/// this crate's own.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
fn read_saber_extra(bytes: &[u8], offset: usize, mesh: MdlMesh) -> Result<MdlSaber, MdlError> {
    check_slice_in_bounds(bytes, offset, SABER_EXTRA_SIZE, "saber_extra")?;

    let verts_ptr = checked_to_usize(
        read_u32(bytes, offset + saber_offsets::VERTS_PTR)?,
        "saber_verts_ptr",
    )?;
    let uvs_ptr = checked_to_usize(
        read_u32(bytes, offset + saber_offsets::UVS_PTR)?,
        "saber_uvs_ptr",
    )?;
    let normals_ptr = checked_to_usize(
        read_u32(bytes, offset + saber_offsets::NORMALS_PTR)?,
        "saber_normals_ptr",
    )?;
    let gl_pool_vert = read_u32(bytes, offset + saber_offsets::GL_POOL_VERT)?;
    let gl_pool_index = read_u32(bytes, offset + saber_offsets::GL_POOL_INDEX)?;

    // Positions: NUM_SABER_VERTS x vec3 (12 bytes each)
    let mut saber_verts = Vec::with_capacity(NUM_SABER_VERTS);
    if verts_ptr > 0 {
        let byte_size = NUM_SABER_VERTS * 12;
        if verts_ptr + byte_size <= bytes.len() {
            saber_verts = read_vec3_array(bytes, verts_ptr, NUM_SABER_VERTS)?;
        } else {
            crate::trace_warn!(
                verts_ptr,
                byte_size,
                bytes_len = bytes.len(),
                "saber_verts array extends beyond buffer"
            );
        }
    }

    // UVs: NUM_SABER_VERTS x vec2 (8 bytes each)
    let mut saber_uvs = Vec::with_capacity(NUM_SABER_VERTS);
    if uvs_ptr > 0 {
        let byte_size = NUM_SABER_VERTS * 8;
        if uvs_ptr + byte_size <= bytes.len() {
            for i in 0..NUM_SABER_VERTS {
                let base = uvs_ptr + i * 8;
                saber_uvs.push([read_f32(bytes, base)?, read_f32(bytes, base + 4)?]);
            }
        } else {
            crate::trace_warn!(
                uvs_ptr,
                byte_size,
                bytes_len = bytes.len(),
                "saber_uvs array extends beyond buffer"
            );
        }
    }

    // Normals: NUM_SABER_VERTS x vec3 (12 bytes each)
    let mut saber_normals = Vec::with_capacity(NUM_SABER_VERTS);
    if normals_ptr > 0 {
        let byte_size = NUM_SABER_VERTS * 12;
        if normals_ptr + byte_size <= bytes.len() {
            saber_normals = read_vec3_array(bytes, normals_ptr, NUM_SABER_VERTS)?;
        } else {
            crate::trace_warn!(
                normals_ptr,
                byte_size,
                bytes_len = bytes.len(),
                "saber_normals array extends beyond buffer"
            );
        }
    }

    Ok(MdlSaber {
        mesh,
        saber_verts,
        saber_uvs,
        saber_normals,
        gl_pool_vert,
        gl_pool_index,
    })
}

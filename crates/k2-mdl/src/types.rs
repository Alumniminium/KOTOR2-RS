//! Node-specific data types for the MDL format.
//!
//! Each MDL node carries a base header (name, transform, controllers) plus
//! type-specific data determined by the node's bitflags. This module defines
//! the [`MdlNodeData`] enum that discriminates between node types, along with
//! the concrete structs for each variant.
//!
//! ## Rust Representation
//!
//! The engine's C++ code uses an inheritance hierarchy (`MdlNodeSkin` extends
//! `MdlNodeTriMesh` extends `MdlNode`). This maps to a flat enum with
//! composition: mesh-derived variants hold an [`MdlMesh`] field rather
//! than inheriting from a base class, so the enum discriminant replaces
//! the C++ vtable and shared mesh data is reached through the composed
//! struct.
//!
//! ```text
//! MdlNodeData (enum)
//!  |-- Base                        (no extra data)
//!  |-- Light(MdlLight)             (92 extra bytes)
//!  |-- Emitter(MdlEmitter)         (224 extra bytes)
//!  |-- Camera(MdlCamera)           (no extra data)
//!  |-- Reference(MdlReference)     (36 extra bytes)
//!  |-- Mesh(MdlMesh)               (332 extra bytes)
//!  |-- Skin(MdlSkin)               (332 + 100 extra bytes)
//!  |     `-- contains MdlMesh      (composition, not inheritance)
//!  |-- AnimMesh(MdlAnimMesh)       (332 + 56 extra bytes)
//!  |     `-- contains MdlMesh
//!  |-- Dangly(MdlDangly)           (332 + 28 extra bytes)
//!  |     `-- contains MdlMesh
//!  |-- Aabb(MdlAabb)               (332 + 4 extra bytes)
//!  |     `-- contains MdlMesh
//!  `-- Saber(MdlSaber)             (332 + 20 extra bytes)
//!        `-- contains MdlMesh
//! ```
//!
//! See [`super`] module docs for the binary format layout and
//! `docs/src/internals/mdl_deep_dive.md` for Ghidra-verified allocation sizes.

/// Type-specific payload for an MDL node.
///
/// Determined at parse time from the node's [`super::node_flags`] bitfield.
/// Mesh-derived variants (Skin, AnimMesh, Dangly, Aabb, Saber) each contain
/// an [`MdlMesh`] plus type-specific extension fields.
#[derive(Debug, Clone, PartialEq)]
pub enum MdlNodeData {
    /// Pure hierarchy node with no type-specific data (flags = 0x0001).
    Base,
    /// Light node (flags & 0x0002).
    Light(MdlLight),
    /// Emitter node (flags & 0x0004).
    Emitter(MdlEmitter),
    /// Camera node (flags & 0x0008).
    Camera(MdlCamera),
    /// Reference to an external model (flags & 0x0010).
    Reference(MdlReference),
    /// Triangle mesh node (flags & 0x0020, no subtype flags).
    Mesh(MdlMesh),
    /// Skinned mesh with bone weights (flags & 0x0060).
    Skin(MdlSkin),
    /// Animated mesh with per-frame vertex sets (flags & 0x00A0).
    AnimMesh(MdlAnimMesh),
    /// Dangly mesh with physics constraints (flags & 0x0120).
    Dangly(MdlDangly),
    /// AABB walkmesh tree (flags & 0x0220).
    Aabb(MdlAabb),
    /// Lightsaber blade mesh (flags & 0x0820).
    Saber(MdlSaber),
}

impl MdlNodeData {
    /// Returns the binary flags for this node type.
    ///
    /// These are the on-disk bitflags written at node offset `0x00`.
    /// Derived from the variant rather than stored independently, ensuring
    /// the type system and binary representation stay in sync.
    ///
    /// See [`super::node_flags`] for the individual flag constants.
    #[must_use = "flags returns a computed value and has no side effects"]
    pub fn flags(&self) -> u32 {
        use super::node_flags;
        match self {
            MdlNodeData::Base => node_flags::HEADER,
            MdlNodeData::Light(_) => node_flags::HEADER | node_flags::LIGHT,
            MdlNodeData::Emitter(_) => node_flags::HEADER | node_flags::EMITTER,
            MdlNodeData::Camera(_) => node_flags::HEADER | node_flags::CAMERA,
            MdlNodeData::Reference(_) => node_flags::HEADER | node_flags::REFERENCE,
            MdlNodeData::Mesh(_) => node_flags::HEADER | node_flags::MESH,
            MdlNodeData::Skin(_) => node_flags::HEADER | node_flags::MESH | node_flags::SKIN,
            MdlNodeData::AnimMesh(_) => node_flags::HEADER | node_flags::MESH | node_flags::ANIM,
            MdlNodeData::Dangly(_) => node_flags::HEADER | node_flags::MESH | node_flags::DANGLY,
            MdlNodeData::Aabb(_) => node_flags::HEADER | node_flags::MESH | node_flags::AABB,
            MdlNodeData::Saber(_) => node_flags::HEADER | node_flags::MESH | node_flags::SABER,
        }
    }

    /// Returns a reference to the contained mesh, if this is a mesh-derived variant.
    #[must_use = "mesh returns a reference and has no side effects"]
    pub fn mesh(&self) -> Option<&MdlMesh> {
        match self {
            MdlNodeData::Mesh(m) => Some(m),
            MdlNodeData::Skin(s) => Some(&s.mesh),
            MdlNodeData::AnimMesh(a) => Some(&a.mesh),
            MdlNodeData::Dangly(d) => Some(&d.mesh),
            MdlNodeData::Aabb(a) => Some(&a.mesh),
            MdlNodeData::Saber(s) => Some(&s.mesh),
            _ => None,
        }
    }

    /// Returns a mutable reference to the contained mesh, if this is a mesh-derived variant.
    #[must_use = "mesh_mut returns a reference and has no side effects"]
    pub fn mesh_mut(&mut self) -> Option<&mut MdlMesh> {
        match self {
            MdlNodeData::Mesh(m) => Some(m),
            MdlNodeData::Skin(s) => Some(&mut s.mesh),
            MdlNodeData::AnimMesh(a) => Some(&mut a.mesh),
            MdlNodeData::Dangly(d) => Some(&mut d.mesh),
            MdlNodeData::Aabb(a) => Some(&mut a.mesh),
            MdlNodeData::Saber(s) => Some(&mut s.mesh),
            _ => None,
        }
    }
}

/// A face in the binary MaxFace format (32 bytes on disk).
///
/// Contains geometric data for rendering and collision: a plane equation,
/// surface material ID, face adjacency indices, and vertex indices.
///
/// Layout verified via Ghidra struct `MaxFace` (32 bytes, `/KotOR Types/Rendering`)
/// and cross-checked with the `0x1A` vertex-index offset constant used in both
/// `MdlNodeDanglyMesh::InternalGenVertices` and `MdlNodeAnimMesh::InternalGenVertices`.
/// See `docs/src/internals/mdl_deep_dive.md` "TriMesh".
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MdlFace {
    /// Face plane normal vector (3 x f32). MaxFace offset +0x00.
    pub plane_normal: [f32; 3],
    /// Face plane distance from origin (f32). MaxFace offset +0x0C.
    pub plane_distance: f32,
    /// Surface material/type ID (u32). MaxFace offset +0x10.
    pub surface_id: u32,
    /// Indices of the three adjacent faces (3 x u16). MaxFace offset +0x14.
    pub adjacent: [u16; 3],
    /// Vertex indices into the mesh vertex array (3 x u16). MaxFace offset +0x1A.
    pub vertex_indices: [u16; 3],
}

/// A triangle mesh contained within a node.
///
/// Vertex attribute data comes from the companion MDX file. Each attribute
/// is stored in a separate array, matching the interleaved MDX vertex layout
/// controlled by the `mdx_vertex_flags` bitfield and per-attribute byte offsets
/// in the TriMesh header (+0x100..+0x120).
///
/// See `docs/src/internals/mdl_deep_dive.md` "MDX vertex layout" for the full specification.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdlMesh {
    // --- Toolset function pointer stubs (extra +0x00, +0x04) ---
    // These are stale code addresses from BioWare's build toolset binary,
    // baked into the MDL file during serialization. The engine overwrites
    // them at load time with its own function pointers. They have no
    // runtime meaning, but serve as a toolset version fingerprint:
    // all vanilla K1 files share the same pair per mesh subtype.
    //
    // See `docs/src/internals/mdl_deep_dive.md` "TriMesh".
    /// `gen_vertices` function pointer stub (u32). Extra +0x00.
    pub fn_ptr_gen_vertices: u32,
    /// `remove_temporary_array` function pointer stub (u32). Extra +0x04.
    pub fn_ptr_remove_temp_array: u32,

    // --- Inline scalar/fixed-size fields (Ghidra-verified) ---
    // See `docs/src/internals/mdl_deep_dive.md` "TriMesh".
    /// Bounding box minimum corner (3xf32). Extra +0x14.
    pub bounding_min: [f32; 3],
    /// Bounding box maximum corner (3xf32). Extra +0x20.
    pub bounding_max: [f32; 3],
    /// Bounding sphere radius (f32). Extra +0x2C.
    pub bsphere_radius: f32,
    /// Bounding sphere center (3xf32). Extra +0x30.
    pub bsphere_center: [f32; 3],
    /// RGB diffuse color (3xf32). Extra +0x3C.
    pub diffuse_color: [f32; 3],
    /// RGB ambient color (3xf32). Extra +0x48.
    pub ambient_color: [f32; 3],
    /// Transparency hint (0=opaque, 1=transparent). Extra +0x54.
    pub transparency_hint: i32,
    /// Primary texture name (up to 32 chars). Extra +0x58.
    pub texture_0: String,
    /// Secondary/lightmap texture name (up to 32 chars). Extra +0x78.
    pub texture_1: String,
    /// UV animation enable flag. Extra +0xE8.
    pub animate_uv: i32,
    /// UV animation direction X. Extra +0xEC.
    pub uv_direction_x: f32,
    /// UV animation direction Y. Extra +0xF0.
    pub uv_direction_y: f32,
    /// UV jitter amount. Extra +0xF4.
    pub uv_jitter: f32,
    /// UV jitter speed. Extra +0xF8.
    pub uv_jitter_speed: f32,
    /// Number of UV texture channels. Extra +0x132.
    pub texture_channel_count: u16,
    /// Lightmapped flag. Extra +0x134.
    pub light_mapped: bool,
    /// Rotate texture flag. Extra +0x135.
    pub rotate_texture: bool,
    /// Background geometry flag. Extra +0x136.
    pub is_background_geometry: bool,
    /// Beaming flag. Extra +0x138.
    pub beaming: bool,
    /// Total surface area (computed by toolset). Extra +0x13C.
    pub total_surface_area: f32,

    // --- MDX vertex attribute arrays ---
    // Each array has `vertex_count` elements when populated, or is empty if
    // the attribute is not present (offset == -1 in the mesh header).
    /// Vertex positions (3xf32: x, y, z). MDX flag 0x01.
    pub positions: Vec<[f32; 3]>,
    /// Vertex normals (3xf32: x, y, z). MDX flag 0x20.
    pub normals: Vec<[f32; 3]>,
    /// Vertex colors (4xu8: R, G, B, A). No flag bit - offset != -1 indicates presence.
    pub vertex_colors: Vec<[u8; 4]>,
    /// Primary UV coordinates (2xf32: u, v). MDX flag 0x02.
    pub uv1: Vec<[f32; 2]>,
    /// Secondary UV coordinates (2xf32: u, v). MDX flag 0x04.
    pub uv2: Vec<[f32; 2]>,
    /// Tertiary UV coordinates (2xf32: u, v). MDX flag 0x08.
    pub uv3: Vec<[f32; 2]>,
    /// Quaternary UV coordinates (2xf32: u, v). MDX flag 0x10.
    pub uv4: Vec<[f32; 2]>,
    /// Tangent space basis (3x3xf32: tangent, bitangent, cross). MDX flag 0x80.
    ///
    /// Each entry is 36 bytes: three vec3s forming the tangent-space basis
    /// used for bump/normal mapping.
    pub tangent_space: Vec<[[f32; 3]; 3]>,

    /// Face data parsed from the MaxFace array (32 bytes per face on disk).
    pub faces: Vec<MdlFace>,

    // --- TriMesh internal CExoArrayList fields (+0x98..+0xC8) ---
    // These 5 CExoArrayList slots form a GL index buffer submission system.
    // Three are derivable (face_count*3, packed u16 face indices, content
    // pointer); one is a dead field (always zeros in KotOR); one stores the
    // "inverted counter" mesh sequence value.
    //
    // See `docs/src/formats/models/mesh_derived_fields.md` for full documentation.
    /// Mesh sequence "inverted counter" value from `index_buffer_pools` (+0xC8).
    ///
    /// Preserved from binary on roundtrip. For newly constructed models, compute
    /// via the inverted counter formula (see `mesh_derived_fields.md` section 1.5).
    /// The engine overwrites this at load time with a GL pool handle.
    pub inverted_counter: u32,

    /// Whether the source binary used the "embedded position" variant for the
    /// `vertex_indices_count` payload (+0xB0).
    ///
    /// When true, the writer emits a 4-byte tag + `vertex_count * 12` bytes
    /// of position data instead of a single u32 `face_count * 3`.
    pub has_embedded_positions: bool,

    /// TriMesh shared index offset scalar (+0xD4).
    pub shared_index_offset: i32,
    /// TriMesh shared index pool scalar (+0xD8).
    pub shared_index_pool: i32,
    /// TriMesh shared index size scalar (+0xDC).
    pub shared_index_size: i32,
    /// TriMesh indices-per-face scalar (+0xE0). Vanilla value is typically 3.
    pub indices_per_face: u32,
    /// Number of vertices declared in the mesh header.
    pub vertex_count: u16,
    /// Is this mesh renderable? (MdlNodeTriMesh +0x189).
    pub render: bool,
    /// Does this mesh cast shadows? (MdlNodeTriMesh +0x187).
    pub shadow: bool,
    /// KotOR2-only fields. `None` for K1 models.
    pub tsl: Option<MdlMeshTsl>,
}

/// The 8 bytes KotOR2 inserts into the TriMesh header after `render`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MdlMeshTsl {
    /// Dirt (grime overlay) enabled.
    pub dirt_enabled: bool,
    /// Dirt texture index.
    pub dirt_texture: u16,
    /// Dirt coordinate space (0 = local, else world).
    pub dirt_coord_space: u16,
    /// Hide this mesh when the model is drawn as a hologram.
    pub hide_in_holograms: bool,
}

impl MdlMesh {
    /// Compute the axis-aligned bounding box from vertex positions.
    ///
    /// Returns `(bounding_min, bounding_max)`, or `None` if positions are empty.
    #[must_use]
    pub fn compute_bounding_box(&self) -> Option<([f32; 3], [f32; 3])> {
        let mut iter = self.positions.iter();
        let first = iter.next()?;
        let mut min = *first;
        let mut max = *first;
        for pos in iter {
            for i in 0..3 {
                if pos[i] < min[i] {
                    min[i] = pos[i];
                }
                if pos[i] > max[i] {
                    max[i] = pos[i];
                }
            }
        }
        Some((min, max))
    }

    /// Compute the centroid-based bounding sphere from vertex positions.
    ///
    /// Uses the engine's algorithm from `PartTriMesh::GetMinimumSphere`
    /// (`0x00443330`): center = centroid, radius = max distance to any vertex.
    /// This is NOT the true minimum bounding sphere (Welzl), but matches
    /// vanilla file values.
    ///
    /// Returns `(center, radius)`, or `None` if positions are empty.
    #[must_use]
    pub fn compute_bounding_sphere(&self) -> Option<([f32; 3], f32)> {
        if self.positions.is_empty() {
            return None;
        }
        // Precision loss impossible: KotOR vertex counts fit easily in f64's 53-bit mantissa.
        #[allow(clippy::cast_precision_loss, clippy::as_conversions)]
        let n = self.positions.len() as f64;
        let mut cx = 0.0f64;
        let mut cy = 0.0f64;
        let mut cz = 0.0f64;
        for pos in &self.positions {
            cx += f64::from(pos[0]);
            cy += f64::from(pos[1]);
            cz += f64::from(pos[2]);
        }
        // Intentional f64->f32 narrowing: geometry averaging is computed in double
        // precision but stored as f32 per the MDL format.
        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let center = [(cx / n) as f32, (cy / n) as f32, (cz / n) as f32];
        let max_dist_sq = self
            .positions
            .iter()
            .map(|pos| {
                let dx = pos[0] - center[0];
                let dy = pos[1] - center[1];
                let dz = pos[2] - center[2];
                dx * dx + dy * dy + dz * dz
            })
            .fold(0.0f32, f32::max);
        Some((center, max_dist_sq.sqrt()))
    }

    /// Compute the total surface area of all triangles in the mesh.
    ///
    /// Returns 0.0 if positions or faces are empty.
    #[must_use]
    pub fn compute_total_surface_area(&self) -> f32 {
        let positions = &self.positions;
        if positions.is_empty() {
            return 0.0;
        }
        let total: f64 = self
            .faces
            .iter()
            .filter_map(|face| {
                let v0 = usize::from(face.vertex_indices[0]);
                let v1 = usize::from(face.vertex_indices[1]);
                let v2 = usize::from(face.vertex_indices[2]);
                if v0 >= positions.len() || v1 >= positions.len() || v2 >= positions.len() {
                    return None;
                }
                let p0 = positions[v0];
                let p1 = positions[v1];
                let p2 = positions[v2];
                let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                let [cx, cy, cz] = cross_f32_as_f64(e1, e2);
                Some((cx * cx + cy * cy + cz * cz).sqrt() * 0.5)
            })
            .sum();
        // Intentional f64->f32 narrowing: cross-product magnitude sums are computed
        // in double precision but the result is stored as f32.
        #[allow(clippy::cast_possible_truncation, clippy::as_conversions)]
        let result = total as f32;
        result
    }

    /// Recompute `plane_normal` and `plane_distance` for all faces from vertex positions.
    ///
    /// Uses the standard cross-product formula:
    /// ```text
    /// normal = normalize(cross(v1 - v0, v2 - v0))
    /// distance = -dot(normal, v0)
    /// ```
    /// Degenerate triangles (zero-area) get a zero normal and zero distance.
    pub fn recompute_face_planes(&mut self) {
        let positions = &self.positions;
        for face in &mut self.faces {
            let v0 = usize::from(face.vertex_indices[0]);
            let v1 = usize::from(face.vertex_indices[1]);
            let v2 = usize::from(face.vertex_indices[2]);
            if v0 >= positions.len() || v1 >= positions.len() || v2 >= positions.len() {
                face.plane_normal = [0.0; 3];
                face.plane_distance = 0.0;
                continue;
            }
            let p0 = positions[v0];
            let p1 = positions[v1];
            let p2 = positions[v2];
            let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
            let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
            let [nx, ny, nz] = cross_f32(e1, e2);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            if len > 0.0 {
                face.plane_normal = [nx / len, ny / len, nz / len];
                face.plane_distance = -(face.plane_normal[0] * p0[0]
                    + face.plane_normal[1] * p0[1]
                    + face.plane_normal[2] * p0[2]);
            } else {
                face.plane_normal = [0.0; 3];
                face.plane_distance = 0.0;
            }
        }
    }

    /// Compute face adjacency from vertex positions using position-based edge matching.
    ///
    /// For each edge of each face, finds the adjacent face sharing that edge.
    /// Uses `0xFFFF` as the no-neighbor sentinel. Position matching uses `{:.4e}`
    /// formatting for floating-point key comparison (handles duplicate vertices
    /// at the same position with different normals/UVs).
    ///
    /// For non-manifold edges (more than 2 faces sharing an edge), the smallest
    /// face index is used (deterministic, matches PyKotor behavior).
    pub fn compute_face_adjacency(&mut self) {
        use std::collections::HashMap;

        let positions = &self.positions;
        if positions.is_empty() || self.faces.is_empty() {
            return;
        }

        // Step 1: Build position key for each vertex.
        let pos_key = |idx: usize| -> String {
            if idx >= positions.len() {
                return String::new();
            }
            let p = positions[idx];
            format!("{:.4e},{:.4e},{:.4e}", p[0], p[1], p[2])
        };

        // Step 2: Group vertices by position.
        let mut vertex_group: HashMap<String, Vec<usize>> = HashMap::new();
        for i in 0..positions.len() {
            vertex_group.entry(pos_key(i)).or_default().push(i);
        }

        // Step 3: Build vertex-to-faces map.
        let mut vertex_to_faces: HashMap<usize, Vec<usize>> = HashMap::new();
        for (fi, face) in self.faces.iter().enumerate() {
            for &vi in &face.vertex_indices {
                vertex_to_faces.entry(usize::from(vi)).or_default().push(fi);
            }
        }

        // Step 4: For each vertex, collect all faces touching any vertex at the
        // same position (handles UV seam / hard edge duplicates).
        let face_set_for_vertex = |vi: usize| -> Vec<usize> {
            let key = pos_key(vi);
            let mut faces = Vec::new();
            if let Some(group) = vertex_group.get(&key) {
                for &gv in group {
                    if let Some(flist) = vertex_to_faces.get(&gv) {
                        faces.extend_from_slice(flist);
                    }
                }
            }
            faces.sort_unstable();
            faces.dedup();
            faces
        };

        // Step 5: For each face and each edge, find the adjacent face.
        let face_count = self.faces.len();
        let mut adjacency = vec![[0xFFFFu16; 3]; face_count];

        for (fi, face) in self.faces.iter().enumerate() {
            let vis = face.vertex_indices;
            let edges = [
                (usize::from(vis[0]), usize::from(vis[1])),
                (usize::from(vis[1]), usize::from(vis[2])),
                (usize::from(vis[2]), usize::from(vis[0])),
            ];
            for (ei, &(va, vb)) in edges.iter().enumerate() {
                let faces_a = face_set_for_vertex(va);
                let faces_b = face_set_for_vertex(vb);

                // Intersect face sets, excluding self.
                let mut best = None;
                let (mut ia, mut ib) = (0, 0);
                while ia < faces_a.len() && ib < faces_b.len() {
                    match faces_a[ia].cmp(&faces_b[ib]) {
                        std::cmp::Ordering::Less => ia += 1,
                        std::cmp::Ordering::Greater => ib += 1,
                        std::cmp::Ordering::Equal => {
                            let candidate = faces_a[ia];
                            if candidate != fi {
                                // Use min (smallest index) for deterministic non-manifold handling.
                                if best.is_none() {
                                    best = Some(candidate);
                                }
                            }
                            ia += 1;
                            ib += 1;
                        }
                    }
                }

                if let Some(adj) = best {
                    if adj <= 0xFFFE {
                        adjacency[fi][ei] =
                            u16::try_from(adj).expect("adj <= 0xFFFE guarantees it fits u16");
                    }
                }
            }
        }

        // Apply computed adjacency.
        for (fi, adj) in adjacency.into_iter().enumerate() {
            self.faces[fi].adjacent = adj;
        }
    }

    /// Recompute all derivable geometric fields at once.
    ///
    /// Updates: bounding box, bounding sphere, total surface area,
    /// face plane normals/distances, and face adjacency.
    pub fn recompute_derived_fields(&mut self) {
        if let Some((bmin, bmax)) = self.compute_bounding_box() {
            self.bounding_min = bmin;
            self.bounding_max = bmax;
        }
        if let Some((center, radius)) = self.compute_bounding_sphere() {
            self.bsphere_center = center;
            self.bsphere_radius = radius;
        }
        self.total_surface_area = self.compute_total_surface_area();
        self.recompute_face_planes();
        self.compute_face_adjacency();
    }
}

/// Light node data.
///
/// Binary layout: 92 extra bytes after the base node header.
/// Contains scalar properties and flare data arrays.
///
/// Verified via `MdlNodeLight::MdlNodeLight` constructor (`0x0044a3f0`),
/// `InputBinary::ResetLight` (`0x004a05e0`), and Ghidra struct
/// `MdlNodeLight` (172 bytes total = 80 base + 92 extra).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, PartialEq)]
pub struct MdlLight {
    /// Flare radius (f32, default 0.0). Extra offset +0x00.
    pub flare_radius: f32,
    /// Texture SafePointers (runtime-only). Extra +0x04.
    ///
    /// Three u32 values populated at runtime by `AurTextureGetReference`
    /// after resolving texture names. In vanilla binaries these contain
    /// stale addresses from the build toolset. Preserved for lossless
    /// roundtrip and toolset fingerprinting (same rationale as the
    /// mesh function pointer stubs).
    pub texture_safe_ptrs: [u32; 3],
    /// Flare sizes (one f32 per flare). Extra +0x10 CExoArrayList pointer.
    pub flare_sizes: Vec<f32>,
    /// Flare positions (one f32 per flare). Extra +0x1C CExoArrayList pointer.
    pub flare_positions: Vec<f32>,
    /// Flare color shifts (one vec3 per flare). Extra +0x28 CExoArrayList pointer.
    pub flare_color_shifts: Vec<[f32; 3]>,
    /// Flare texture names. Extra +0x34 CExoArrayList pointer.
    ///
    /// Stored as a CExoArrayList of char* pointers, where each pointer is
    /// also relocated and points to a null-terminated string. Flattened to
    /// a `Vec<String>` for ergonomic access.
    pub flare_texture_names: Vec<String>,
    /// Light priority (default 5). Extra offset +0x40.
    pub priority: i32,
    /// Dynamic type count (default 1). Extra offset +0x44.
    pub num_dynamic_types: i32,
    /// Affects dynamic objects (default 1). Extra offset +0x48.
    pub affectdynamic: i32,
    /// Casts shadow (default 1). Extra offset +0x4C.
    pub shadow: i32,
    /// Ambient-only light (default 0). Extra offset +0x50.
    pub ambientonly: i32,
    /// Generate flare effect (default 0). Extra offset +0x54.
    pub generateflare: i32,
    /// Fading light (default 1). Extra offset +0x58.
    pub fading_light: i32,
}

impl Default for MdlLight {
    /// Returns a light node with engine default values.
    ///
    /// Defaults match the `MdlNodeLight::MdlNodeLight` constructor (`0x0044a3f0`):
    /// priority=5, num_dynamic_types=1, affectdynamic=1, shadow=1, fading_light=1.
    fn default() -> Self {
        MdlLight {
            flare_radius: 0.0,
            texture_safe_ptrs: [0u32; 3],
            flare_sizes: vec![],
            flare_positions: vec![],
            flare_color_shifts: vec![],
            flare_texture_names: vec![],
            priority: 5,
            num_dynamic_types: 1,
            affectdynamic: 1,
            shadow: 1,
            ambientonly: 0,
            generateflare: 0,
            fading_light: 1,
        }
    }
}

/// Emitter node data.
///
/// Binary layout: 224 extra bytes (0xE0) after the 80-byte base node header.
/// All fields are inline fixed-size data with no pointers - no relocation
/// needed (only `ResetMdlNodeParts` is called, not a type-specific Reset).
///
/// The `update` field is architecturally significant: it determines which
/// runtime emitter class is instantiated ("Fountain", "Explosion", "Single",
/// or "Lightning"). Controller 502 ("detonate") is only valid for "Explosion"
/// emitters.
///
/// Verified via `MdlNodeEmitter` constructor (`0x0044a300`, 304 bytes total),
/// `MdlNodeEmitter::InternalParseField` (`0x00469700`, 2,632 bytes),
/// and `MdlNodeEmitter::InternalCreateInstance` (`0x0049d5c0`).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, PartialEq)]
pub struct MdlEmitter {
    /// Dead space / unused float (default 0.0). Extra offset +0x00.
    pub deadspace: f32,
    /// Blast radius (default 0.0). Extra offset +0x04.
    pub blast_radius: f32,
    /// Blast length (default 0.0). Extra offset +0x08.
    pub blast_length: f32,
    /// Number of branches (default 0). Extra offset +0x0C.
    pub num_branches: i32,
    /// Control point smoothing (default 0). Extra offset +0x10.
    pub control_pt_smoothing: i32,
    /// X grid size (default 0). Extra offset +0x14.
    pub x_grid: i32,
    /// Y grid size (default 0). Extra offset +0x18.
    pub y_grid: i32,
    /// Spawn type (default 0). Extra offset +0x1C.
    pub spawn_type: i32,
    /// Emitter update type string (up to 32 chars). Extra offset +0x20.
    ///
    /// Determines which runtime emitter class is instantiated:
    /// - `"Fountain"` - steady particle effects (most common)
    /// - `"Explosion"` - burst effects (supports controller 502 "detonate")
    /// - `"Single"` - one-shot particle
    /// - `"Lightning"` - lightning bolt effects
    pub update: String,
    /// Render type string (up to 32 chars). Extra offset +0x40.
    pub render: String,
    /// Blend type string (up to 32 chars). Extra offset +0x60.
    pub blend: String,
    /// Texture name (up to 32 chars). Extra offset +0x80.
    pub texture: String,
    /// Chunk name (up to 16 chars). Extra offset +0xA0.
    pub chunk_name: String,
    /// Two-sided texturing (default 0). Extra offset +0xB0.
    pub two_sided_tex: i32,
    /// Loop emitter (default 0). Extra offset +0xB4.
    pub loop_emitter: i32,
    /// Render order (default 0). Extra offset +0xB8.
    pub render_order: u16,
    /// Frame blending enabled (default false). Extra offset +0xBA.
    pub frame_blending: bool,
    /// Depth texture name (up to 16 chars). Extra offset +0xBB.
    pub depth_texture_name: String,
    /// Reserved/padding bytes at extra offset +0xCB..+0xE0 (21 bytes).
    /// Preserved verbatim for roundtrip fidelity per the reserved field rule.
    pub reserved: [u8; 21],
}

impl Default for MdlEmitter {
    /// Returns an emitter with engine default values.
    ///
    /// All numeric fields default to zero, all strings default to empty.
    fn default() -> Self {
        MdlEmitter {
            deadspace: 0.0,
            blast_radius: 0.0,
            blast_length: 0.0,
            num_branches: 0,
            control_pt_smoothing: 0,
            x_grid: 0,
            y_grid: 0,
            spawn_type: 0,
            update: String::new(),
            render: String::new(),
            blend: String::new(),
            texture: String::new(),
            chunk_name: String::new(),
            two_sided_tex: 0,
            loop_emitter: 0,
            render_order: 0,
            frame_blending: false,
            depth_texture_name: String::new(),
            reserved: [0u8; 21],
        }
    }
}

/// Camera node data.
///
/// Camera nodes have no type-specific binary data beyond the base 80-byte
/// node header. Confirmed via `ParseFieldDispatch` dispatch table - camera
/// (0x009) routes to the base `MdlNode::InternalParseField`, not a
/// camera-specific parser.
///
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdlCamera {
    _private: (),
}

impl MdlCamera {
    /// Creates a camera node (no type-specific data).
    pub fn new() -> Self {
        MdlCamera { _private: () }
    }
}

impl Default for MdlCamera {
    fn default() -> Self {
        Self::new()
    }
}

/// Reference node data.
///
/// References an external model by name, with an optional reattachment flag.
/// Binary layout: 36 extra bytes after the base node header.
///
/// Verified via `MdlNodeReference::InternalParseField` (`0x00468070`) and
/// Ghidra struct `MdlNodeReference` (116 bytes total = 80 base + 36 extra).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MdlReference {
    /// Referenced model name (up to 32 chars). Extra offset +0x00.
    pub ref_model: String,
    /// Whether this reference can be reattached (default 0). Extra offset +0x20.
    pub reattachable: i32,
}

/// Skinned mesh with bone weight data.
///
/// Extends [`MdlMesh`] with inverse bind pose data and bone index mappings
/// used for skeletal animation. Each bone has an inverse bind rotation
/// (quaternion) and translation (vector) that transform from bone space
/// to model space.
///
/// Binary layout: 100 extra bytes (0x64) after the 332-byte TriMesh header.
/// Verified via `InputBinary::ResetSkin` (`0x004a01b0`) and Ghidra struct
/// `MdlNodeSkin` (512 bytes total = 412 TriMesh + 100 extra).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, PartialEq)]
pub struct MdlSkin {
    /// The base triangle mesh.
    pub mesh: MdlMesh,
    /// MDX per-vertex bone weights byte offset (i32). Skin extra +0x0C.
    ///
    /// Byte offset within each MDX vertex stride to the 4-float bone weights.
    /// Value -1 means not present.
    pub mdx_bone_weights_offset: i32,
    /// MDX per-vertex bone indices byte offset (i32). Skin extra +0x10.
    ///
    /// Byte offset within each MDX vertex stride to the 4-float bone indices.
    /// Value -1 means not present.
    pub mdx_bone_indices_offset: i32,
    /// Per-vertex bone weights (4 x f32, one per influence slot).
    ///
    /// Each vertex has up to 4 bone influences. Unused slots are zero.
    /// One entry per vertex when populated, empty when no skin weights are present.
    pub bone_weights: Vec<[f32; 4]>,
    /// Per-vertex bone indices (4 x f32, one per influence slot).
    ///
    /// Stored as f32 by engine convention (small non-negative integers).
    /// Each index references a bone in the bonemap. Unused slots are zero.
    /// One entry per vertex when populated, empty when no skin weights are present.
    pub bone_indices: Vec<[f32; 4]>,
    /// Bone-to-node index mapping from skin extra +0x14 pointer.
    ///
    /// Each u32 maps a local bone index to a global skeleton node index.
    /// The engine relocates the pointer when the count (extra +0x18) > 0.
    pub bonemap: Vec<u32>,
    /// Inverse bind rotation per bone (quaternion: w, x, y, z).
    ///
    /// One entry per bone. Transforms from bone space to model space.
    /// Data pointed to by CExoArrayList at skin extra +0x1C.
    pub qbone_ref_inv: Vec<[f32; 4]>,
    /// Inverse bind translation per bone (vector: x, y, z).
    ///
    /// One entry per bone. Transforms from bone space to model space.
    /// Data pointed to by CExoArrayList at skin extra +0x28.
    pub tbone_ref_inv: Vec<[f32; 3]>,
    /// Bone constant indices mapping local bone index to global skeleton node index.
    ///
    /// Data pointed to by CExoArrayList at skin extra +0x34.
    pub bone_constant_indices: Vec<i32>,
    /// Fixed-size array of 16 bone node serial numbers (u16). Skin extra +0x40.
    ///
    /// Maps up to 16 local bone slots to node indices in the model hierarchy.
    /// Unused slots are zero.
    pub bone_node_numbers: [u16; 16],
}

impl Default for MdlSkin {
    /// Defaults with MDX bone offsets set to -1 (not present).
    fn default() -> Self {
        Self {
            mesh: MdlMesh::default(),
            mdx_bone_weights_offset: -1,
            mdx_bone_indices_offset: -1,
            bone_weights: Vec::new(),
            bone_indices: Vec::new(),
            bonemap: Vec::new(),
            qbone_ref_inv: Vec::new(),
            tbone_ref_inv: Vec::new(),
            bone_constant_indices: Vec::new(),
            bone_node_numbers: [0u16; 16],
        }
    }
}

/// Animated mesh with per-frame vertex sets.
///
/// Extends [`MdlMesh`] with animated vertex positions and texture coordinates
/// that vary over time. The `sample_period` controls the animation playback rate.
///
/// Binary layout: 56 extra bytes (0x38) after the 332-byte TriMesh header.
/// Note: `ResetAnim` processes the extra data BEFORE calling `ResetTriMeshParts`
/// (reversed order vs other subtypes).
///
/// Verified via `InputBinary::ResetAnim` (`0x004a0060`) and Ghidra struct
/// `MdlNodeAnimMesh` (468 bytes total = 412 TriMesh + 56 extra).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdlAnimMesh {
    /// The base triangle mesh.
    pub mesh: MdlMesh,
    /// Animation sampling period (f32). AnimMesh extra +0x00.
    pub sample_period: f32,
    /// Animated vertex positions (Vector: x, y, z per entry).
    ///
    /// Data pointed to by CExoArrayList at anim extra +0x04.
    pub anim_verts: Vec<[f32; 3]>,
    /// Animated texture coordinates (Vector: u, v, w per entry).
    ///
    /// Data pointed to by CExoArrayList at anim extra +0x10.
    pub anim_t_verts: Vec<[f32; 3]>,
    // --- Runtime-only fields (always zero in authored files) ---
    // These 6 fields have no ASCII parser names (confirmed via
    // `MdlNodeAnimMesh::InternalParseField` at 0x0046a240).
    // They are populated at runtime by `InternalGenVertices`.
    // Preserved for roundtrip fidelity.
    /// Runtime data pointer 1 (u32, relocated if `data_count_1 != 0`). Extra +0x1C.
    pub data_ptr_1: u32,
    /// Size guard for `data_ptr_1` (u32). Extra +0x20.
    pub data_count_1: u32,
    /// Padding (u32). Extra +0x24.
    pub padding_24: u32,
    /// Runtime animated vertices pointer (u32). Extra +0x28.
    pub anim_vertices_ptr: u32,
    /// Runtime animated texture vertices pointer (u32). Extra +0x2C.
    pub anim_tex_vertices_ptr: u32,
    /// Count for `anim_vertices_ptr` (u32). Extra +0x30.
    pub anim_vertices_count: u32,
    /// Count for `anim_tex_vertices_ptr` (u32). Extra +0x34.
    pub anim_tex_vertices_count: u32,
}

/// Dangly mesh with physics constraints.
///
/// Extends [`MdlMesh`] with per-vertex constraint weights and physics parameters
/// that control dangling geometry behavior (hair, cloaks, banners, etc.).
///
/// Binary layout: 28 extra bytes (0x1C) after the 332-byte TriMesh header.
/// Verified via `InputBinary::ResetDangly` (`0x004a0100`) and Ghidra struct
/// `MdlNodeDanglyMesh` (440 bytes total = 412 TriMesh + 28 extra).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdlDangly {
    /// The base triangle mesh.
    pub mesh: MdlMesh,
    /// Per-vertex constraint weights (float array).
    ///
    /// Each value constrains how much a vertex can move (0.0 = fully fixed,
    /// higher = more freedom). Array length matches `mesh.vertex_count`.
    /// Data pointed to by CExoArrayList at dangly extra +0x00.
    pub constraints: Vec<f32>,
    /// Maximum displacement distance (f32). Dangly extra +0x0C.
    pub displacement: f32,
    /// Spring tightness factor (f32). Dangly extra +0x10.
    pub tightness: f32,
    /// Oscillation period (f32). Dangly extra +0x14.
    pub period: f32,
    /// Per-vertex positions for the dangly constraint system (vec3 array).
    ///
    /// Each entry is a 3-component position `[x, y, z]` for one constraint
    /// vertex. Array length matches `mesh.vertex_count`. Pointed to by the
    /// conditional data pointer at dangly extra +0x18 (only present when
    /// `vertex_count > 0`).
    ///
    /// At runtime, the engine copies this array into a GL vertex pool via
    /// `PartDanglyMesh` constructor (`0x00447980`): allocates
    /// `vertex_count * 12` bytes, copies `vertex_count` vec3s from this
    /// pointer, then uploads to GL.
    pub dangly_vertices: Vec<[f32; 3]>,
}

/// AABB walkmesh tree node.
///
/// Extends [`MdlMesh`] with a bounding-volume hierarchy used for
/// walkmesh collision detection. The tree is stored inline in the MDL
/// content blob as a flattened binary tree of axis-aligned bounding boxes.
///
/// Binary layout: 4 extra bytes after the 332-byte TriMesh header.
/// The single field is a pointer to the root AABB tree node.
///
/// Verified via `ResetMdlNode` inline processing and `ResetAABBTree`
/// (`0x004a0260`). See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdlAabb {
    /// The base triangle mesh.
    pub mesh: MdlMesh,
    /// Parsed AABB binary search tree. `None` if the root pointer was null.
    pub aabb_tree: Option<Box<AabbNode>>,
}

/// A node in the AABB binary search tree used for spatial face queries.
///
/// Each node occupies 40 bytes on disk: 6xf32 bounding box, right/left child
/// pointers, face index, and split direction flags. Internal nodes partition
/// space along an axis (encoded in [`split_direction_flags`](AabbNode::split_direction_flags))
/// and have two children. Leaf nodes reference a single face by index.
///
/// Binary layout (40 bytes):
///
/// | Offset | Size | Field |
/// |--------|------|-------|
/// | +0x00 | 12 | `box_min` (3xf32) |
/// | +0x0C | 12 | `box_max` (3xf32) |
/// | +0x18 | 4 | `right_child` (u32, content-relative offset, 0 = leaf) |
/// | +0x1C | 4 | `left_child` (u32, content-relative offset, 0 = leaf) |
/// | +0x20 | 4 | `face_index` (i32, -1 for internal nodes) |
/// | +0x24 | 4 | `split_direction_flags` (u32, axis bitmask) |
///
/// Verified via Ghidra AABB struct (size 40), `operator_new(0x28)` in
/// `Parse_AABB`, and `ResetAABBTree` (`0x004a0260`).
/// See `docs/src/internals/mdl_deep_dive.md` "AABB walkmesh".
#[derive(Debug, Clone, PartialEq)]
pub struct AabbNode {
    /// Axis-aligned bounding box minimum corner.
    pub box_min: [f32; 3],
    /// Axis-aligned bounding box maximum corner.
    pub box_max: [f32; 3],
    /// Face index for leaf nodes (>= 0). Internal nodes use -1.
    pub face_index: i32,
    /// Split plane direction flags (internal nodes only, 0 for leaves).
    ///
    /// Bitmask encoding which axis the split occurred on:
    /// 1=+X, 2=+Y, 4=+Z, 8=-X, 16=-Y, 32=-Z.
    pub split_direction_flags: u32,
    /// Left child subtree (`None` for leaf nodes).
    pub left: Option<Box<AabbNode>>,
    /// Right child subtree (`None` for leaf nodes).
    pub right: Option<Box<AabbNode>>,
}

/// Lightsaber blade mesh.
///
/// Extends [`MdlMesh`] with saber-specific geometry data for 176 fixed
/// vertices. All lightsaber models use exactly `NUM_SABER_VERTS` (176)
/// vertices for blade geometry.
///
/// Binary layout: 20 extra bytes (0x14) after the 332-byte TriMesh header.
/// Contains 3 relocated data pointers for saber vertex data and 2 runtime
/// GL render pool IDs (allocated by `GLRender::RequestPool` at load time,
/// not meaningful in the binary file).
///
/// kotorblender names these `off_saber_verts`, `off_saber_uv`,
/// `off_saber_normals`, `inv_count1` and `inv_count2`. Noted for
/// cross-reference; the identifiers below are this crate's own, and the last
/// two are named for what the fields hold rather than for what they were once
/// mistaken for.
///
/// Verified via `InputBinary::ResetLightsaber` (`0x004a0460`) and
/// `ParseNode` allocation (`operator_new(0x1B0)`).
/// See `docs/src/internals/mdl_deep_dive.md` "Node types in depth".
#[derive(Debug, Clone, Default, PartialEq)]
pub struct MdlSaber {
    /// The base triangle mesh.
    pub mesh: MdlMesh,
    /// Saber vertex positions (176 x vec3). Saber extra +0x00 pointer.
    pub saber_verts: Vec<[f32; 3]>,
    /// Saber texture coordinates (176 x vec2). Saber extra +0x04 pointer.
    pub saber_uvs: Vec<[f32; 2]>,
    /// Saber vertex normals (176 x vec3). Saber extra +0x08 pointer.
    pub saber_normals: Vec<[f32; 3]>,
    /// GL vertex pool ID (runtime-only, stale in binary). Saber extra +0x0C.
    pub gl_pool_vert: u32,
    /// GL index pool ID (runtime-only, stale in binary). Saber extra +0x10.
    pub gl_pool_index: u32,
}

/// Cross product of two 3D vectors in f32.
fn cross_f32(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Cross product of two f32 vectors computed in f64 precision.
fn cross_f32_as_f64(a: [f32; 3], b: [f32; 3]) -> [f64; 3] {
    [
        f64::from(a[1]) * f64::from(b[2]) - f64::from(a[2]) * f64::from(b[1]),
        f64::from(a[2]) * f64::from(b[0]) - f64::from(a[0]) * f64::from(b[2]),
        f64::from(a[0]) * f64::from(b[1]) - f64::from(a[1]) * f64::from(b[0]),
    ]
}

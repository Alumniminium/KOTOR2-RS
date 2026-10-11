//! MDL controller types and keyframe structures.

/// Controller type code identifying which property is animated.
///
/// Stored as a raw `u32` matching the on-disk binary format. Named constants
/// are provided for Ghidra-verified types; unknown codes are preserved
/// losslessly for roundtrip fidelity.
///
/// Controller type codes are node-type-specific - the same numeric value may
/// have different semantic meaning on different node types (e.g., Light vs
/// Emitter). The base node types (Position, Orientation, Scale) are shared
/// across all node types.
///
/// Base node controller codes verified via `MdlNode::InternalParseField`
/// (`0x00465560`). See `docs/src/internals/mdl_deep_dive.md`
/// "Base vs type-specific controllers".
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct MdlControllerType(u32);

impl MdlControllerType {
    // --- Base node controllers (all node types) ---
    // Verified via `MdlNode::InternalParseField` (`0x00465560`).

    /// Position (3 floats: x, y, z). Code 8.
    pub const POSITION: Self = Self(8);
    /// Orientation quaternion (4 floats: w, x, y, z). Code 20.
    pub const ORIENTATION: Self = Self(20);
    /// Uniform scale (1 float). Code 36.
    pub const SCALE: Self = Self(36);

    // --- Mesh node controllers (trimesh, skin, dangly, aabb, saber, animmesh) ---
    // Verified via `MdlNodeTriMesh::InternalParseField` (`0x00469700`).

    /// Self-illumination color (3 floats: r, g, b). Code 100. Mesh context.
    pub const SELFILLUMCOLOR: Self = Self(100);
    /// Alpha transparency (1 float). Code 132. Mesh context.
    pub const ALPHA: Self = Self(132);

    // --- Light node controllers ---
    // Verified via `MdlNodeLight::InternalParseField` (`0x00469150`).

    /// Light color (3 floats: r, g, b). Code 76. Light context.
    pub const COLOR: Self = Self(76);
    /// Light radius (1 float). Code 88. Light context.
    pub const RADIUS: Self = Self(88);
    /// Shadow radius (1 float). Code 96. Light context.
    pub const SHADOW_RADIUS: Self = Self(96);
    /// Vertical displacement (1 float). Code 100. Light context.
    ///
    /// Note: same raw code as `SELFILLUMCOLOR` (mesh) and `DRAG` (emitter).
    pub const VERTICAL_DISPLACEMENT: Self = Self(100);
    /// Light multiplier (1 float). Code 140. Light context.
    pub const MULTIPLIER: Self = Self(140);

    // --- Emitter node controllers ---
    // Verified via `MdlNodeEmitter::InternalParseField` (`0x004658b0`).

    /// Emitter alpha end (1 float). Code 80.
    pub const ALPHA_END: Self = Self(80);
    /// Emitter alpha start (1 float). Code 84.
    pub const ALPHA_START: Self = Self(84);
    /// Emitter birthrate (1 float). Code 88.
    pub const BIRTHRATE: Self = Self(88);
    /// Emitter bounce coefficient (1 float). Code 92.
    pub const BOUNCE_CO: Self = Self(92);
    /// Emitter combine time (1 float). Code 96.
    pub const COMBINETIME: Self = Self(96);
    /// Emitter drag (1 float). Code 100.
    pub const DRAG: Self = Self(100);
    /// Emitter frames per second (1 float). Code 104.
    pub const FPS: Self = Self(104);
    /// Emitter frame end (1 float). Code 108.
    pub const FRAME_END: Self = Self(108);
    /// Emitter frame start (1 float). Code 112.
    pub const FRAME_START: Self = Self(112);
    /// Emitter gravity (1 float). Code 116.
    pub const GRAV: Self = Self(116);
    /// Emitter life expectancy (1 float). Code 120.
    pub const LIFE_EXP: Self = Self(120);
    /// Emitter mass (1 float). Code 124.
    pub const MASS: Self = Self(124);
    /// Emitter point-to-point bezier control 2 (1 float). Code 128.
    pub const P2P_BEZIER2: Self = Self(128);
    /// Emitter point-to-point bezier control 3 (1 float). Code 132.
    pub const P2P_BEZIER3: Self = Self(132);
    /// Emitter particle rotation (1 float). Code 136.
    pub const PARTICLE_ROT: Self = Self(136);
    /// Emitter random velocity (1 float). Code 140.
    pub const RANDVEL: Self = Self(140);
    /// Emitter size start (1 float). Code 144.
    pub const SIZE_START: Self = Self(144);
    /// Emitter size end (1 float). Code 148.
    pub const SIZE_END: Self = Self(148);
    /// Emitter size start Y (1 float). Code 152.
    pub const SIZE_START_Y: Self = Self(152);
    /// Emitter size end Y (1 float). Code 156.
    pub const SIZE_END_Y: Self = Self(156);
    /// Emitter spread (1 float). Code 160.
    pub const SPREAD: Self = Self(160);
    /// Emitter threshold (1 float). Code 164.
    pub const THRESHOLD: Self = Self(164);
    /// Emitter velocity (1 float). Code 168.
    pub const VELOCITY: Self = Self(168);
    /// Emitter X size (1 float). Code 172.
    pub const XSIZE: Self = Self(172);
    /// Emitter Y size (1 float). Code 176.
    pub const YSIZE: Self = Self(176);
    /// Emitter blur length (1 float). Code 180.
    pub const BLUR_LENGTH: Self = Self(180);
    /// Emitter lightning delay (1 float). Code 184.
    pub const LIGHTNING_DELAY: Self = Self(184);
    /// Emitter lightning radius (1 float). Code 188.
    pub const LIGHTNING_RADIUS: Self = Self(188);
    /// Emitter lightning scale (1 float). Code 192.
    pub const LIGHTNING_SCALE: Self = Self(192);
    /// Emitter lightning subdivision (1 float). Code 196.
    pub const LIGHTNING_SUB_DIV: Self = Self(196);
    /// Emitter lightning zigzag (1 float). Code 200.
    pub const LIGHTNING_ZIGZAG: Self = Self(200);
    /// Emitter alpha mid (1 float). Code 216.
    pub const ALPHA_MID: Self = Self(216);
    /// Emitter percent start (1 float). Code 220.
    pub const PERCENT_START: Self = Self(220);
    /// Emitter percent mid (1 float). Code 224.
    pub const PERCENT_MID: Self = Self(224);
    /// Emitter percent end (1 float). Code 228.
    pub const PERCENT_END: Self = Self(228);
    /// Emitter size mid (1 float). Code 232.
    pub const SIZE_MID: Self = Self(232);
    /// Emitter size mid Y (1 float). Code 236.
    pub const SIZE_MID_Y: Self = Self(236);
    /// Emitter random birthrate (1 float). Code 240.
    pub const RANDOM_BIRTH_RATE: Self = Self(240);
    /// Emitter target size (1 float). Code 252.
    pub const TARGET_SIZE: Self = Self(252);
    /// Emitter number of control points (1 float). Code 256.
    pub const NUM_CONTROL_PTS: Self = Self(256);
    /// Emitter control point radius (1 float). Code 260.
    pub const CONTROL_PT_RADIUS: Self = Self(260);
    /// Emitter control point delay (1 float). Code 264.
    pub const CONTROL_PT_DELAY: Self = Self(264);
    /// Emitter tangent spread (1 float). Code 268.
    pub const TANGENT_SPREAD: Self = Self(268);
    /// Emitter tangent length (1 float). Code 272.
    pub const TANGENT_LENGTH: Self = Self(272);
    /// Emitter color mid (3 floats: r, g, b). Code 284.
    pub const COLOR_MID: Self = Self(284);
    /// Emitter color end (3 floats: r, g, b). Code 380.
    pub const COLOR_END: Self = Self(380);
    /// Emitter color start (3 floats: r, g, b). Code 392.
    pub const COLOR_START: Self = Self(392);
    /// Emitter detonate (1 float). Code 502.
    pub const DETONATE: Self = Self(502);

    /// Creates a controller type from a raw type code.
    pub const fn from_raw(code: u32) -> Self {
        Self(code)
    }

    /// Returns the raw type code.
    #[must_use = "raw returns a computed value and has no side effects"]
    pub const fn raw(self) -> u32 {
        self.0
    }
}

impl std::fmt::Debug for MdlControllerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::POSITION => write!(f, "Position({})", self.0),
            Self::ORIENTATION => write!(f, "Orientation({})", self.0),
            Self::SCALE => write!(f, "Scale({})", self.0),
            _ => write!(f, "Unknown({})", self.0),
        }
    }
}

impl std::fmt::Display for MdlControllerType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::POSITION => f.write_str("Position"),
            Self::ORIENTATION => f.write_str("Orientation"),
            Self::SCALE => f.write_str("Scale"),
            _ => write!(f, "Unknown({})", self.0),
        }
    }
}

impl From<u32> for MdlControllerType {
    fn from(code: u32) -> Self {
        Self(code)
    }
}

impl From<MdlControllerType> for u32 {
    fn from(ct: MdlControllerType) -> u32 {
        ct.0
    }
}

/// Bezier flag bit in the raw column_count byte (bit 4).
///
/// When set, the actual value count per keyframe row is
/// `(raw_column_count & 0x0F) * 3` (control point triplets).
pub const CTRL_FLAG_BEZIER: u8 = 0x10;

/// A controller definition for a node.
///
/// Defines a set of animation keys that drive one property over time.
///
/// The binary `NewController` header is 16 bytes. Bytes +0x04..+0x05 and
/// +0x0D..+0x0F are preserved verbatim for roundtrip fidelity.
#[derive(Debug, Clone, PartialEq)]
pub struct MdlController {
    /// The type of property being controlled (Position, Orientation, etc.).
    pub controller_type: MdlControllerType,
    /// The animation keys (time + values).
    pub keys: Vec<MdlKey>,
    /// Raw column_count byte from controller key header +0x0C.
    ///
    /// The lower 4 bits encode the base column count. Bit 4 (0x10) is the
    /// Bezier flag -- when set, each row has 3x the base columns (control
    /// point triplets). Special case: ORIENTATION with raw value 2 means
    /// integral compressed quaternion (1 u32 per row, stored as f32 bits).
    ///
    /// Preserved for roundtrip fidelity; the reader decodes this to
    /// determine the actual value count per keyframe row.
    pub raw_column_count: u8,
    /// Unknown bytes from controller key header +0x04..+0x05.
    ///
    /// Preserved verbatim for roundtrip fidelity per the reserved field rule.
    /// Zero for newly constructed controllers.
    pub key_unknown_04: [u8; 2],
    /// Unknown bytes from controller key header +0x0D..+0x0F.
    ///
    /// Preserved verbatim for roundtrip fidelity per the reserved field rule.
    /// Zero for newly constructed controllers.
    pub key_unknown_0d: [u8; 3],
}

/// A single animation keyframe.
#[derive(Debug, Clone, PartialEq)]
pub struct MdlKey {
    /// Time of the keyframe.
    pub time: f32,
    /// Values associated with this key (e.g., [x, y, z] for Position).
    pub values: Vec<f32>,
}

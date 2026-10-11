# Rendering notes

## Coordinates

- Game space is Z-up. Everything from game data is spawned under one
  `WorldRoot` entity rotated -90 degrees about X, so node and instance
  transforms stay in game units. bevy = (x, z, -y). Helpers live in
  `crates/kotor2/src/coords.rs`.
- MDL node position and orientation come from the node header. The
  orientation is stored w, x, y, z. kotorblender also uses the header values
  for geometry; the controllers repeat them for animation.
- LYT room positions are world offsets. Room models are authored around
  those offsets with identity rotation.
- Face winding in MDL matches Bevy's default CCW front faces after the basis
  change (a proper rotation, so no mirroring).

## Textures

- UVs are used as stored, for both UV1 (diffuse) and UV2 (lightmap), with
  TPC rows read top-down. Lightmaps line up with geometry and diffuse reads
  right, so no V flip is needed.
- TPC header: data_size u32 (0 = uncompressed; otherwise the base level
  size), alpha_test f32, width u16, height u16, pixel_type u8 (1 grey,
  2 RGB/DXT1, 4 RGBA/DXT5), mip_count u8, 114 reserved bytes. The payload is
  the mip chain, repeated per face for cube maps (height == 6 * width). TXI
  text follows the chain.
- Shipped data truncates some mip chains (script_c/script_h). The decoder
  keeps every complete level and zero-pads a short base, never fails.
- DXT is decoded to RGBA8 on the CPU (texpresso), keeping the file's own
  mips. DXT1 alpha is ignored (opaque).
- Textures repeat (address mode Repeat). Many room textures tile.
- Diffuse alpha is only transparency when the TXI does not name
  `envmaptexture`, `bumpyshinytexture` or `bumpmaptexture`. In those cases
  alpha is a reflection or bump mask and the surface is opaque. A sidecar
  `.txi` resource overrides the embedded footer.
- Transparent textures alpha-blend, with a tiny alpha-test cut to drop fully
  clear texels. `blending additive` in the TXI maps to additive blending.
  The kolto tank glass in 101PER is the reference case: alpha-testing at 0.5
  shredded it into holes.

## Lighting

- Room meshes with `light_mapped` and a full UV2 set use texture_1 as the
  lightmap, and the result is diffuse * lightmap. No other lighting is
  applied; this matches the original's static look.
- Meshes without a lightmap get a flat ambient term plus one directional
  term until real dynamic lights land.
- Meshes with no texture use the MDL diffuse colour.

## Shadows (crates/kotor2/src/shadows.rs)

Cause of "no shadows": KMaterial's shader is unlit (lightmap or a fixed
ambient term) and the scene had no shadow-casting light at all, so Bevy
never rendered a shadow map and nothing sampled one. No area data asks for
one either: across all 82 modules the ARE `SunShadows`/`MoonShadows` flag is
false except once, `ShadowOpacity` is 205 everywhere, and the MDL light nodes
parse with `shadow` 0 (`k2probe lights MODULE` lists them). The original
still darkens the floor under creatures, so the shadow light is synthesised.

- One `DirectionalLight` (`ShadowSun`) with shadow maps. Its colour is white
  and its illuminance is the shadow darkness (ShadowOpacity / 255 * 0.9); the
  KMaterial shader reads it from `lights.directional_lights[0].color.r`, so
  the light adds no lighting of its own. The shader multiplies the surface
  colour by `1 - darkness * (1 - visibility)`, only on surfaces facing the
  light (smoothstep on N.L, so the far side of a prop is not blackened by its
  own shadow map) and only on meshes that are not `NotShadowReceiver`.
- Direction: from the nearest model light node (rooms, within 14 m
  horizontally, above the leader) down to the party leader, tilted at most
  0.6 rad from vertical, eased at 4/s when the nearest light changes. With no
  light in reach it is a fixed ray. Cascades: High 3 cascades to 40 m on a
  2048 map, Low 2 cascades to 22 m on 1024, Off no shadow maps.
- Casters: creature, door and placeable meshes whose MDL trimesh `shadow` flag
  is set (`TNode::casts_shadow`; a mesh with the flag clear gets
  `NotShadowCaster`). Rooms never cast (`NotShadowCaster` on every mesh under
  a room root): their lightmaps hold the baked shadows and casting would cost
  every room mesh a shadow pass.
- Skinned casters follow the pose. The shadow pass uses Bevy's prepass vertex
  shader, which defines SKINNED from the joint attributes that `add_skin`
  sets, and skin meshes carry `NoFrustumCulling`, so cascade culling keeps
  them. The flag varies per skin node. Atton's clothing
  overlay (p_attonbb) has no casters; the pmbam body under it casts with its
  Torso, LArm and RArm. Kreia (p_kreiabb) casts with her head, hands and teeth;
  her torso, arms, hood and eyes do not. Pinned by
  `model::tests::companion_pins_current_caster_lists` (prints SKIP without the
  install).
- Mandalore (appearance 462, P_MandaloreBB, modeltype F, supermodel
  S_Female02) is one skinned body. There is no separate head or armour model:
  the head is a skin node of p_mandalorebb, like the torso, larm and rarm. All
  five drawn parts (torso, larm, rarm, head, tubes) are skins. Only `tubes`
  (the back hoses) has the flag set, so the body casts nothing and the shadow
  is the hoses alone.
- Status of the Mandalore result: the test
  `model::tests::mandalore_pins_current_caster_rule` pins the CURRENT rule
  output (the MDL shadow flag honoured on skins). It is not checked against
  the original and waits on the user's in-game look. Across the install's
  chitin and override MDLs, 1152 of 1414 skin nodes (81.5%) have the flag set;
  the flag varies per skin node. The alternative is casting from every rendered
  skin, a one-line change at `casts_shadow` in `node()` (model.rs ~409). It
  is not made.
- The shadow pass needed its own fragment shader (`PREPASS_SRC` in
  material.rs). Bevy's default one writes depth for every triangle, so glass
  and fading meshes would cast solid shadows, and for masked meshes it would
  read the wrong bind group. Ours discards blended (`FLAG_BLEND`, set on the
  GPU copy of the params from `KMaterial::blend`, so runtime fades follow),
  additive and hologram materials, and alpha-cuts masked ones with the diffuse
  alpha (cycle and scroll textures are left uncut).
- Blob shadow: a soft dark disc (`Blob`, 64x64 radial alpha texture, unlit
  StandardMaterial, alpha blend) 3 cm above each creature's root, half-size
  1.15 x the appearance `hitradius`. Alpha 0.6 alone (Low), 0.4 under a real
  shadow (High), hidden at Off.
- Setting: `GraphicsSettings.shadows` (graphics.rs, `ShadowQuality`),
  `soft_shadows` picks Gaussian filtering on the camera. `K2_SHADOWS=off|low|high`
  overrides it for a run. `K2_FRAMELOG=1` logs the mean frame time every 120
  frames; test windows on workspace 4 are paced at 60 Hz by the compositor, so
  that number cannot show the shadow cost (all three levels read 16.6 ms).
  The shadow pass only draws creature, door and placeable meshes.

Before/after: `kotor2 --module 261tel --cam -56.3,41.9,19.9,157.6,-20
--screenshot out.png --frames 300` with `K2_SHADOWS=off` versus unset
(character on snow: body and limbs cast a shadow); `--module 262tel --cam
6.5,41.5,23.5,180,-25` (interior floor). 101per (kolto tank) changes only
around the character; no acne on rooms or glass.

## Known gaps

Environment maps, sky domes, ARE fog, emitters,
dangly physics, and the walkmesh AABB node (skipped, not rendered).

## TXI blending punchthrough

Head textures (e.g. pmha01, DXT5 with header alpha_test ~0.98) carry
`blending punchthrough`: alpha is a cutout, not a blend. Rendering them as
alpha-blended skips depth writes, and per-mesh sorting then draws the mouth
interior (same texture) over the closed lips. Punchthrough is now opaque with
an alpha test at 0.5; hair fringe cutouts stay clean.

## Additive blending (fixed)

Bevy's AlphaMode::Add is premultiplied blending and expects alpha 0 from
the fragment shader (its own PBR shader does this). KMaterial returned
alpha 1, so every TXI `blending additive` surface (glass, glows) was
drawn opaque. Additive materials now set FLAG_ADDITIVE; the shader outputs
rgb * a with alpha 0. See docs/notes/minigames.md (turret cockpit glass).

## Animated materials

TXI `proceduretype cycle` (frame grid), `water` (approximated UV warp), mesh
`animate_uv` scrolling and per-node alpha clips are animated; see
"Force cages and animated materials" in animation.md.

## Creature bodies and armour (verified in game and viewer)

- All armour bodies (pmb/pfb a..n, n_commm/f, p_*bb) have one rendered skin
  mesh set sharing a single texture; the per-limb helper meshes are
  render=false. So the creature texture override (`CreatureSpec.texture`,
  applied to every mesh) is safe for them, and wrong only when the override
  is not the model's own family.
- Bug fixed: appearance rows with a body model but an empty `texL` (Atton
  P_AttonBB, Mira, Visas, Bao-Dur, Ond soldiers ...) got `texa` + 01 forced
  over the model, so Atton in basic clothes wore the pmbamc01 body skin
  (flat green/pink). Empty `texL` now means no override. Before/after:
  viewer `p_attonbb --head p_attonh --tex pmbamc01` vs no `--tex`.
- Head gear: i_mask_NNN attached at the head's MaskHook (armor.rs). Before,
  equipped helmets/goggles/masks were never drawn.
- Droids and creature models (modeltype F/S/L) never go through the armour
  letters; party droids with armour items stay unchanged (checked HK-47,
  T3-M4 next to a helmeted heavy-armour PC).
- Male and female PCs (appearance 578 / 663), light (a_light_02), heavy
  (a_heavy_03) and robe (a_robe_05) all resolve to pmb*/pfb* with the item's
  TextureVar and render without missing or doubled parts.

## Projectiles (crates/kotor2/src/projectiles.rs)

Draws what k2-world announces as `WorldEvent::Projectile` (combat.md,
"Projectiles"); the world sets launch, flight and outcome.

- Bolt: two crossed additive quads, 0.12 x 1.2 m, long axis along the flight
  (the path is in Bevy space, so the root is rotated from Y to the direction).
  Texture: the bolt model's first emitter texture (Fx_laser_01 for the
  blaster); the material is a KMaterial with the emitter's Lighten blend
  (Blend::Additive plus FLAG_ADDITIVE, as emitter.rs does). Light: a PointLight
  (4000, range 3, no shadows) tinted by the bolt model's light node colour key
  (controller 76), red if the model has none.
- Muzzle flash: the ammunitiontypes `muzzleflash` model (v_muzflash_01) under
  the flight root with FxLive; its `impact` clip is started once the animation
  player exists (PendingClip), and its detonate events fire the emitters.
- Thrown or launched item: the item model (spells.2da projmodel, spawn_tree,
  no FxLive) on the path. A grenade is lobbed (peak height 0.25 x distance + 0.8
  m, at most 4 m) and tumbles; a rocket or dart flies straight along its
  direction.
- Landing: an additive sphere (0.3 m radius, emissive, grows 0.5x to 2x over
  0.25 s) with a PointLight (20000 for a hit or a deflection, 60000 for an
  explosion). A detonation spawns the visualeffects explosion model with FxLive
  and its `impact` clip, kept 12 s (particles keep their own lifetimes), and
  plays `soundimpact` at the landing through Voice::play_sound_at (range 4 to 40).
- Deflect: the flight turns into a 0.4 s bounce 12 m back toward the shooter.
- Hooks: a bolt starts at the weapon's `bullethook` (searched under the
  held-weapon roots); a thrown item at the shooter's `rhand`. A shooter with no
  placed root launches nothing.

Not verified in the game yet: the streak look, the light values, the arc
height, the flash sizes and where explosions sit against walls. Tests:
`cargo test -p kotor2 projectiles` (flight logic only, no assets).

## Thrown lightsaber (crates/kotor2/src/saber_throw.rs)

The saber is the item model (w_lghtsbr_001), spawned with its AnimPlayer; its
`throwout` clip loops as the spin. It flies along a path of waypoints (hand,
each stop, hand) at the times the world sends; a flash at each stop. The held
saber (children of the rhand node) is Visibility::Hidden while it is out. The
catch clip (catchsab) is set through CreatureAnim.scripted, as cast_visuals
does for the throw. Not verified in the game: the spin and the path look, the
flash, and where the return lands. Blade core not drawn (see combat.md).



## Lightsaber blades (model.rs saber_blade)

The blade is the MDL Saber node (w_lghtsbr_001: two Saber nodes, plane242 and
plane239, at z 0.18, the second turned 90 degrees about z; texture
w_lsabreblue01, red w_lsabrered01). Its geometry is saber_verts (176), with
saber_uvs and saber_normals; the base mesh's 12 faces (over vertices 0-33, mostly
slivers) are not used. Only pieces 0-7 and 88-95 of the 22 eight-vertex pieces have area
in the bind pose: a three-quad strip each, 0.2 m wide across the two, from
z -0.043 to 0.98 m. saber_blade draws those strips with reone's strip pattern
(kSaberPieceFaceIndices, seedhartha/reone, GPL-3), the back piece reversed, and
checks that every face has area and faces its saber normal (the model test). The
two crossed nodes make a flat cross about 1 m long. model.rs drew only the plain
glow sheets (34 vertices) before. Not verified in the game yet: the blade's look
and its additive blend (the texture's TXI), and that the powered-off state is
hidden.

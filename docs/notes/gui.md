# GUI (.gui layouts, bitmap fonts) - crates/kotor2/src/gui/

The game's own screens are drawn from its .gui resources (GFF "GUI ",
type 2047, gui.bif; the Aspyr override/ dir replaces a few, e.g.
mainmenu8x6_p, equip_p, saveload_p).

## Layout files

- The root struct is a control (TGuiPanel). CONTROLS lists the children.
  Child EXTENTs are relative to the root's LEFT/TOP: pause_p's root sits at
  (195, 96) and its children start at (2, 2). Most screens have the root at
  0,0 and 800x600.
- Variants: `_p` PC, `_x` Xbox. Some ship per resolution (mainmenu8x6_p,
  10x7, 12x9, 16x12; mipc28x6_p .. mipc216x12_p; tooltip*). We always load
  the 800x600 one (`<base>8x6_p`, then `<base>_p`) and scale it.
- K2's HUD is `mipc28x6_p` (the "2" set; `mi8x6_p` is the K1-style one).
  In-game menus pair a screen (inventory_p, equip_p, ...) with `top_p`
  (screen tabs and the party portraits). Esc opens `optionsingame_p`;
  the pause box is `pause_p`; dialog is `dialog_p`.

## Control fields

- CONTROLTYPE: 2 panel, 4 label, 5 label-with-hilight, 6 button,
  7 toggle button, 8 slider, 9 scrollbar, 10 progress bar, 11 list box.
- TAG, ID, EXTENT {LEFT, TOP, WIDTH, HEIGHT}.
- BORDER / HILIGHT / SELECTED / HILIGHTSELECTED / PROGRESS: {CORNER, EDGE,
  FILL, FILLSTYLE, DIMENSION, INNEROFFSET(Y), COLOR, PULSING}. Fill sits
  inside the frame (inset by DIMENSION). EDGE art is a top edge: bottom is
  a vertical mirror, left/right are the strip turned a quarter. CORNER art
  is the top-left corner, mirrored for the others. COLOR tints fill, edges
  and corners: the menu icons are white art tinted teal (BORDER) and pale
  yellow (HILIGHT). Empty resref or "0" means none.
- TEXT {FONT, TEXT, STRREF (0xFFFFFFFF none), ALIGNMENT, COLOR, PULSING}.
  ALIGNMENT is a bitfield: 1 left, 2 hcenter, 4 right; 8 top, 16 vcenter,
  32 bottom (9, 10, 17, 18, 20, 34 occur). Hovered buttons draw their text
  in the HILIGHT colour.
- Progress: CURVALUE, MAXVALUE, STARTFROMLEFT (0 even on bars that grow
  from the left). Taller-than-wide bars fill from the bottom. The PROGRESS
  fill is drawn with the matching part of the texture, not squashed.
- List box: PROTOITEM (the row template; its EXTENT is in root space, so
  the row offset is proto - list), PADDING between rows, SCROLLBAR {THUMB,
  DIR images, MAXVALUE, VISIBLEVALUE, CURVALUE}, LEFTSCROLLBAR, LOOPING.
- MOVETO {UP, DOWN, LEFT, RIGHT}: gamepad focus order. Parsed into
  `Control::move_to` (indices into CONTROLS, -1 none). No focus traversal
  reads it yet, but `GuiDef::swap_places` (layout.rs) rewrites it: the two
  controls trade rects and each neighbour's MOVETO is renamed to follow the
  swap. The Editor button relies on this: menus.rs `editor_slot` swaps the
  Exit and More Games places and renames the More Games control BTN_EDITOR.
- Many labels carry designer sample text ("ABCDEFGHIJKLMNOPQRST", "99",
  "CLASS 1", "9999999"); the engine overwrites it. Menu screens blank any
  non-strref label text until the screen fills it.
- Scroll-down arrow buttons reuse the up-arrow art; the engine flips it
  (we flip fills of controls whose tag contains DOWN).

## Textures

TPC rows are stored bottom-up (OpenGL order; 3D UVs rely on that), so GUI
copies flip the base level. TGA comes from rakata already top-left. Only
the base mip is used.

## Fonts

A font is a TPC whose TXI has `numchars 256`, `fontheight`,
`baselineheight`, `texturewidth`, `spacingR`, `spacingB`, then
`upperleftcoords 256` and `lowerrightcoords 256` lines of "u v 0". V = 1 is
the top of the texture. Units are 100 per texel at native size
(texturewidth 2.56 = 256 px), so dialogfont16x16 has 16 px lines; glyph
boxes are tight (I is 4 px, W 15 px, space 12 px). Codes are Windows-1252.
Fonts in use: dialogfont16x16 (menus, HUD), dialogfont10x10 (small HUD
labels, stack counts), fnt_d16x16 (dialog lines and replies).

## Scaling

Layouts are 800x600. Scale s = min(W/800, H/600) (window logical px).
- Placement::Center: the layout scaled by s and centred (menus).
- Placement::Anchor (HUD): each control keeps size*s but is pinned to the
  left/centre/right third and top/middle/bottom third of the window by
  where its centre sits in the layout, so the HUD hugs the corners on wide
  windows (reone does the same with a half-way test).

## Runtime API (gui/mod.rs)

- `open_screen(commands, res, name, placement, z) -> Entity`; the entity
  carries `GuiScreen`.
- `GuiScreen` setters mark one control dirty and only it is redrawn:
  set_visible, set_enabled, set_selected, set_text, set_fill,
  set_text_color, set_progress, set_progress_color, set_screen_pos
  (floating controls), hide_prefixed. `control(tag)` gives the node entity.
- Lists: `set_items(tag, Vec<String>)` or `set_list(tag, Vec<ListItem>)`
  with `ListItem::text(..).with_icon(Some(resref)).with_count(Some(n))`.
  Icon rows draw the icon in a square at the left and the text after it.
  `set_list_selected`, `list_selected`, `hovered_item`. Clicking a row
  selects it. Mouse wheel scrolls. A list box with `set_text` and no rows
  shows wrapped text (description panes).
- `GuiClick { screen, tag, item }` messages for button/toggle/row clicks.
- `BitmapText` component: free-standing bitmap text that sizes its own
  node (used by the dialog UI).

## Behaviour wired so far

- Main menu (mainmenu8x6_p): New Game and Quit; Load/Movies/Music/Options
  disabled; the warp list, Aspyr promo buttons and DLC label hidden.
- HUD (mipc28x6_p): party portraits (portraits.2da row by appearance) with
  vitality/force bars, menu buttons, action bar frames, mode toggles, the
  floating target name (red for hostiles) and health bar over the hovered
  target or the creature being fought. Event-only controls start hidden;
  the minimap is hidden (no area maps yet). Hidden in conversations and
  menus.
- Menus: HUD/top-bar buttons and I/J/M open screens with top_p; Esc closes
  or opens the options menu (Close, Exit Game; the rest disabled); menus
  pause the game. Space or TB_PAUSE pauses with the pause_p box.
- Dialog: NPC line in the top letterbox bar, numbered replies in the
  bottom bar, dialog_p fonts and colours.
- Door message box (dialog_ui.rs, confirm_p.gui): a door's own line (no
  speaker, or the door itself, owner is a Door) with text and no real reply
  (only [Continue] or none) is a box, not the conversation screen. The box shows
  the text in LB_MESSAGE with BTN_OK; BTN_CANCEL is hidden. No letterbox and no
  camera move. OK, Enter or Space takes the next node (advance_conversation, so
  a chain of such lines goes on in turn, each node's scripts running as it is
  entered); the last node ends the talk normally. Escape ends it as aborted
  (end_conversation(true), so the abort hook runs). menus.rs does not open the
  pause menu on Escape while a box is up. ConversationView.boxed and
  ConversationInfo.boxed (control API) report it. A box is not speech:
  LineState.boxed makes speaking_left() None, so remote's speech state does
  not count it. A line with a named speaker, or with a real reply, stays a
  conversation. A door's bare line (no reply links at all, such as meddoor's)
  with text starts a conversation, not a bark, so it can be a box. Empty door
  lines are not boxes: they advance by themselves, as before.

Test hooks: `K2_GUI_OPEN=inventory` opens a menu at start,
`K2_GUI_GIVE=template[:n],...` adds items to the party inventory,
`K2_GUI_TARGET=1` labels the interactable nearest the screen centre (no
mouse in screenshots). `K2_GFF_MAXLIST=999 k2probe gff NAME.gui` dumps a
whole layout.

## Bark bubble (barkbubble_p.gui)

Barks (one-liner conversations, cutscenes.md) use barkbubble_p.gui, which has
one panel and one label. Root TGuiPanel at LEFT 48, TOP 80, 462 x 118 (800 x
600 layout space), border CORNER uibit_brdr_16bct, EDGE uibit_brdr_16bet,
FILL uibit_fill_2bt, DIMENSION 16, COLOR (0.05, 0.35, 0.27). Label
LBL_BARKTEXT at (5, 5), 452 x 108, font fnt_d16x16, ALIGNMENT 18 (centred
horizontally and vertically), COLOR (0.8, 0.8, 0.698). barkbubble_x.gui is the
Xbox variant (unused). We open it with Placement::Center: the root panel (the
border) is skipped for Placement::Anchor, which is how the HUD works. The
whole layout can be moved with `GuiScreen::set_origin` (layout px for the
root's left, top corner); barks use it for nBarkX / nBarkY.

## Game screens (gui/screens/)

Each screen fills its layout from k2-world every frame (setters only
redraw what changed) and turns clicks into world calls. Opened from the
menu bar / keys (I inventory, U equipment, C character, K abilities,
L messages, J journal, M map; Esc closes) or by world requests:
ContainerOpened -> container_p, OpenStore -> store_p, UpgradeScreen ->
upgradesel_p / upgrade_p, ItemCreateScreen -> component_p (workbench) or
chemical_p (lab). Those six are "standalone" (no top_p bar); small panels
authored at 0,0 (container_p is 305x327) are re-centred. Closing them
(any way) calls close_container / close_store / close_craft_screen.
A container opened while a conversation runs (its talk on use, kre_body) is
held and its window opens when ConversationEnded arrives, if the talk was not
aborted and the container is still valid and the open one (inventory.open_container).
A hold that opens nothing closes the container in the world (close_container),
so it does not block saving. A ModuleEntered drops a held window. While the
hold runs, World::save_blocked and exit_save_blocked say "in conversation"
(no window is shown); a destroyed open container is cleared by World::destroy.
The top bar shows the selected party member; BTN_CHANGE2/3 rotate it.

- inventory_p: filters All/Datapads/Weapons/Armor/Useable/Quests/Misc
  (baseitems label "datapad", plot flag, equip slot mask, CastSpell
  property); Use Item runs `World::use_item` (CastSpell property ->
  spells.2da impactscript with GetSpellId / GetSpellTargetObject /
  GetSpellCastItem answering; medpac = spell 64, k_sup_healing).
- equip_p: BTN_INV_* slots (0 head, 1 body, 3 hands, 4/5 weapons,
  7/8 arms, 9 implant, 10 belt). Hovering a slot previews what fits it
  and a slot click pins that list (row 0 unequips, a row click equips).
  The list comes from `World::equip_candidates(creature, slot)` (party
  inventory items passing `can_equip`: slot mask, droid/organic,
  two-handed blocks the left hand). The hover preview outlives the pointer
  leaving the slot while it is over the list, or for 0.8 s, so a row can be
  reached; the hovered slot wins over a pinned one. The description pane
  replaces the slot grid while a row is hovered. The Config 1 / Config 2 slot pairs
  (BTN_INV_WEAP_R/L and R2/L2) are the two weapon sets; they keep their
  meaning when the sets are swapped. BTN_SWAPWEAPONS (relabelled "Switch
  Weapons (Set N)" with the active set) calls swap_weapon_sets.
  LBL_TOHITR/L attack bonus, LBL_ATKR/L damage range, LBL_DEF defense.
- character_p: abilities, vitality/force, defense, saves (class
  savingthrowtable + ability mod), XP and exptable.2da next level.
- abilities_p: skills (skills.2da, rank + key ability modifier), powers
  and feats via progression's known_powers / known_feats.
- journal_p: global.jrl categories x World::journal state; End entries
  are completed. Sort by update time (stamped in sheet_call when
  AddJournalQuestEntry runs), name, priority, planet.
- messages_p: dialog history (NPC lines from conversation_view, player
  replies recorded by choose_reply), feedback (items, skill checks,
  journal, XP, level up), combat rolls, effects (Effect* names split into
  good / bad).
- map_p and the HUD minimap: lbl_map<area> with ARE Map points. NorthAxis
  0/1: u from world X, v from world Y; 2/3 swapped; scale per axis from
  the two point pairs (u, v are texture fractions, v down). Notes are
  waypoints with HasMapNote (lbl_mapcircle, BTN_UP/DOWN cycle); the
  leader is mm_barrow, others friendlyarrow, rotated by facing. The
  minimap draws the texture at native size inside LBL_MAPVIEW (clipped),
  scrolled to centre the leader; LBL_MAP itself is not used (anchoring
  would misplace it). Clicking it opens the map.
- container_p: click a row to take it, Get Items takes all and closes.
- store_p: buy list / sell list (Show Sell List toggles), All / Weapons /
  Armor / Misc filters, prices from the world's markup/markdown.
- upgradesel_p: upgradable party items (inventory + equipment) with
  Ranged/Melee/Armor/Lightsaber filters; when item creation is also
  allowed the first row opens component_p (the layout has no create
  button). upgrade_p: 3 slots (6 for sabers, _LS controls); a slot click
  opens the list of fitting upgrades (row 0 removes); Back closes the
  list, then the screen.
- component_p / chemical_p: create tabs map to itemcreate.2da groups
  (0 armour, 1 ranged, 2 melee, 3 saber, 4 misc) and chemicalcreate.2da
  groups (0 health, 1 stims, 2 mines, 3 grenades); the workbench's
  View Breakdown Items toggles to break down (yield preview per item).

List boxes clip their children (long descriptions no longer spill).
UI sounds (uisfx): hover Entered_Default, panel added/removed, errors,
Checkbox_Check on filters, Inventory_Add/Select/Drop, Upgrade,
Quest_New/Complete_Notify, Level_Up_Notify.

Screenshot test hooks (env): K2_GUI_OPEN, K2_GUI_GIVE, K2_GUI_USE=TAG,
K2_GUI_STORE=TAG, K2_GUI_SCRIPT=NAME, K2_GUI_GLOBAL=name:n,
K2_GUI_CREDITS=n, K2_GUI_JOURNAL=plot:state, K2_GUI_HOVER=BTN_INV_xxx
(equip_p: act as if the mouse sat on that slot), and
K2_GUI_CLICKS=screen:TAG[#row],... (one synthetic click per 30 frames
from frame 120).

## Game over and level up (gameover.rs, progress_ui.rs)

- `GameOver::is_open()` (gameover.rs) is true while the screen is due or up.
  `trigger` makes it due 3 s after PartyDefeated, `show` opens the gameover
  layout, `release` takes it down once the leader stands again (a load).
- `LevelUpUi::is_open()` (progress_ui.rs) is true while a creature is being
  levelled: begin_level_up sets it; applying the level (BTN_STEPNAME5) or Back
  clears it (a banked level starts the next at once), and so does Esc or
  another menu.
- remote.rs passes both into `screen::Look` (remote/screen.rs). `screen_now`
  checks, in order: movie, game over, loading, message box, conversation,
  level up, container, other window, paused, play. A verb the screen does not
  take is refused and the reply names what is open and how to clear it.

## Graphics options (gui/options.rs, graphics.rs)

Esc -> Graphics opens optgraphics_p (no top bar). The three screens edit the
live `GraphicsSettings` resource (graphics.rs) at once; graphics.rs applies
it to the window and cameras whenever it changes.

- optgraphics_p: SLI_GAMMA (Brightness), CB_SHADOWS, CB_GRASS,
  CB_FORCESPEED, BTN_RESOLUTION (popup optresolution_p, LB_RESOLUTIONS: a
  Fullscreen row, then windowed sizes from a standard list plus the
  monitors' video modes, plus the current size; OK applies, Cancel closes),
  BTN_ADVANCED, Close (back to the options menu), Default (this screen's
  fields).
- optgraphicadv_p (file name has no "s"): BTN_TEXQUAL, BTN_ANTIALIAS,
  BTN_ANISOTROPY (button cycles, the LEFT/RIGHT buttons step and clamp),
  CB_FRAMEBUFF, CB_SOFTSHADOWS, CB_VSYNC. OK keeps, Cancel restores the
  values the screen opened with, Default resets its fields. Both go back to
  optgraphics_p.
- While a control is hovered LB_DESC shows the game's own help text
  (strrefs 48707..48716). `K2_GUI_HOVER=TAG` fakes the hover.
- Sliders: Kind::Slider is clickable; holding the mouse sends `GuiSlide
  { screen, tag, value 0..1 }` (gui/mod.rs slider_drag) and the thumb
  (THUMB image) follows `set_progress`.
- The checkbox art is a pill: "uibit_checkbxoff" empty, "uibit_checkbxon"
  with a white X (the SELECTED frame).

What each setting does: resolution / fullscreen (borderless on the current
monitor) write the Window; V-Sync switches AutoVsync / AutoNoVsync; MSAA is
the camera `Msaa` (1/2/4/8, the original's 6x is not a Bevy option);
anisotropy is the sampler clamp, applied to new textures and re-applied to
every loaded one; texture quality drops 0/1/2 top mip levels of textures
loaded afterwards (never below 64 px, procedure textures excepted), so it
shows from the next area load; Brightness is the camera ColorGrading gamma
(slider 50 = 1.0, 0 = 0.62, 100 = 1.62); Soft Shadows is
`ShadowFilteringMethod` Gaussian / Hardware2x2; Shadows is
`GraphicsSettings::shadows` (Off / Low / High) for the renderer (the toggle
sets Off or High). Grass, Force Speed Effects and Frame Buffer Effects are
stored and persisted but nothing consumes them yet (no grass pass, no force
speed effect, no frame buffer passes).

Settings file: `settings.json` in `~/.local/share/kotor2-rs/`, or
`$K2_SETTINGS_DIR/settings.json`. Missing fields and bad values fall back to
the defaults one by one. It is read in main() before the window exists
(size, fullscreen, vsync) and written when the player leaves the graphics
screens or the game quits. `--screenshot` runs without K2_SETTINGS_DIR
neither read nor write it. Log lines: "graphics: settings from ...", "window
size", "present mode", "cameras msaa", "anisotropy", "saved".

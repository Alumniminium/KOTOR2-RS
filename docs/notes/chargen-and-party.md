# Character generation and party management

## Character generation (k2-world chargen.rs, kotor2 chargen_ui.rs)

New Game (main menu) opens K2's own chargen screens:

    classsel_p -> qorcpnl_p (Quick / Custom) -> maincg_p + quickpnl_p | custpnl_p
      Quick:  Portrait (portcust_p), Name (name_p), Play
      Custom: Portrait, Attributes (abchrgen_p), Skills (skchrgen_p),
              Feats (ftchrgen_p, then pwrlvlup_p when level 1 grants powers),
              Name, Play

Steps unlock in order; Play needs all of them. Play stores the result as
`World::player_spec` (PlayerSpec: gender, class, portrait row, appearance,
abilities, skill ranks, feats, powers, name) and starts the prologue. The PC
is built at module entry: level 0 of the class, then level 1 through the
progression level up with the spec's picks (falls back to the recommended
picks if they no longer validate).

Rules (from the game data; reone's chargen confirms the mapping):
- classsel_p BTN_SEL1..6 = male Consular, Sentinel, Guardian, female
  Guardian, Sentinel, Consular. One-line summaries are dialog.tlk
  48031..48033; quick/custom explanations 241/242; ability descriptions
  222..227.
- Body: portraits.2da row (forpc = 1, sex) -> Consular appearance_s,
  Guardian appearance_l, Sentinel appearancenumber.
- Attributes: 30 points over 8 each, max 18; raising a score costs its
  current modifier clamped to 1..=3. Recommended = classes.2da str..cha.
- Level 1 skill points: (skillpointbase + Int mod, min 1) x 2. The game's own
  class descriptions say "Skill Points (*2 at 1st level)". Level 1 HP is
  hit die + Con, Force points Force die + Wis.
- Name: Random uses the game's LTR Markov tables (humanm/humanf + humanl),
  cumulative probabilities per context.
- 000_PLAYER_GENDER is set at PC creation; a_bet3m4 also sets it from
  GetGender(GetFirstPC()).

Flags and hooks:
- `--quickstart`: New Game skips chargen (male Jedi Guardian, first portrait,
  recommended picks). `--newgame` and `--module` also use those defaults when
  no spec exists (the headless World default stays the old level 1 Guardian).
- `K2_NEWGAME_MODULE=103per`: New Game enters another module (to see the PC,
  who sits out the 001ebo prologue). Note: entering 101per this way stays
  black (its a_fadeonload expects the 001ebo hand-over); 103per works.
- K2_GUI_CLICKS now works in the menu too, so chargen can be scripted, e.g.
  `mainmenu:BTN_NEWGAME,classsel:BTN_SEL6,qorcpnl:CUST_CHAR_BTN,...`.

## Party (k2-world party.rs, kotor2 party_ui.rs)

- ShowPartySelectionGUI(exit, forced1, forced2, allowCancel) records a
  `PartySelectRequest`; the screen (partyselect_p) answers with
  `apply_party_selection(&[npc])` or `cancel_party_selection()`, and then the
  exit script runs (before, it ran immediately).
- Roster: available NPC slots 0..=11 with name, level and portrait (the UTC
  PortraitId, else an appearance match). At most two followers; forced slots
  are always in.
- Members left behind keep their state: the object moves to the slot's
  `stored` field and comes back on the next selection (tested: HP survives).
- Leader: `cycle_party_leader` (Tab) rotates control through the standing
  members in the field; `set_party_leader` (HUD BTN_CHAR2/3) picks one. A
  `LeaderChanged` event is emitted; the Bevy side moves the Player marker and
  camera with the leader. The previous leader falls back to party AI.
- HUD and character screens take portraits from `World::portrait_resref`
  (chargen pick / UTC PortraitId / appearance match).
- Test hooks: K2_PARTY_NPCS="6:p_kreia,1:p_baodur", K2_PARTY_JOIN="6,1",
  K2_PARTYSELECT=1, K2_PARTY_TAB=SECS. K2 slots: 0 Atton, 1 Bao-Dur,
  2 Mandalore, 3 G0-T0, 4 Handmaiden, 5 HK-47, 6 Kreia, 7 Mira, 8 T3-M4,
  9 Visas, 10 Hanharr, 11 Disciple.

## Party AI and Force powers

Why Kreia only attacked: no script sets her NPC AI style, so GetNPCAIStyle
returned 0 (NPC_AISTYLE_DEFAULT_ATTACK, marked deprecated in nwscript.nss).
That path picks a Jedi combo at random; combos 1..9 are melee and need no
validation, and the Force combos (13..28) each need specific powers Kreia's
early set (Push, Fear, Speed Burst, Resist Energy, Camouflage) does not
cover, so she always ended on melee. K2's party styles route Jedi through
GN_RunJediSupportAIRoutine (heal, buff, AOE, then Force attacks).

Unset styles now default by role: party Jedi NPC_AISTYLE_PARTY_SUPPORT (13),
other party members NPC_AISTYLE_PARTY_AGGRO (9), everyone else 0. The
original engine's default for companions is not documented; this is a
choice. Result: `k2world powers 852nih` has Kreia open with Speed Burst.

Harness: `K2_CALLS=1 K2_CALLS_RET=1` adds return values to the call log.

# Audio

Code: `crates/kotor2/src/audio.rs`. Bevy audio (rodio) with the `mp3` and
`wav` features on top of the default vorbis.

## File variants (Aspyr 1.0.2)

Every ".wav" in the install falls into one of these. `sniff` detects them by
content, never by extension:

- `riff+mp3`: streamvoice/ (17098) and streammusic/ (160). A 58-byte RIFF
  header that claims PCM (format tag 1), with MPEG layer III frames from 0x3A
  to the end. Hand the bytes from 0x3A to the MP3 decoder.
- `junk470+wav`: streamsounds/ (830) and 10 chitin WAVs. A 470-byte block that
  starts like an MP3 frame (ff f3 60 c4), then a real RIFF PCM WAV.
- `wav`: the other 2255 chitin WAVs (sounds.bif), plain PCM.
- No IMA ADPCM (0x11) or MP3-in-WAV (0x55) files ship. Both are handled anyway
  (ADPCM is expanded to 16-bit PCM by `ima_adpcm_to_pcm`) because mods use
  them.
- Module capsules contain no WAVs.

`kotor2 --audio-survey` sniffs and decodes all 20353 files (31.7 h of audio)
in about 1.5 s with 0 failures. Durations come from parsing the stream itself
(MP3 frame scan, WAV data length), not from the fake headers.

Bevy's `Decodable for AudioSource` unwraps the rodio decoder, so a bad stream
would panic the game. `decode_check` opens every clip once under
`catch_unwind` before it is cached.

## Layout and lookup

- streamvoice/ is nested: `<module number | avo | gbl>/<dialog or speaker>/
  <resref>.wav`, e.g. `101/101kreia/...` or `001/3cfd/0013cfd003.wav`. The
  directories carry no meaning for lookup, so we index the whole tree by
  lowercase file stem.
- streammusic/ and streamsounds/ are flat. Ambient "beds" (`bed_101per`) live
  in streammusic/.
- `SoundIndex::find` order: override/module WAV resources first, then the
  stream dirs (order depends on the kind: music, ambient/effect, voice), then
  chitin WAVs.
- Dialog VO: 11946 of 18030 VO_ResRef/Sound values across all module DLGs
  resolve. The rest (e.g. `0013cfd040`) do not exist anywhere in the
  install, typically droid or cut lines. Lines with no playable file fall
  back to the alien set (below) or, failing that, to a text-based delay.

## Alien dialog voices

Non-human speakers have no recorded lines of their own. Their speech is a
shared generic set, keyed by race:

- DLG top-level `AlienRaceOwner` and per-node `AlienRaceNode` are rows of
  alienvo.2da. Hanharr is row 12, Aqualish row 1, Rodian male row 18. Margar
  (Onderon) has AlienRaceOwner 1 and the appearance Alien_Walrusman.
- Node `Emotion` is a row of emotion.2da. Its label, lowercased, is the
  alienvo.2da column (`Comment_Generic_Short` -> `comment_generic_short`).
  Row 28 (Silence) has no clip.
- The alienvo cell (`AVO_AquaCmntM`) names clips `avo_aquacmntm1`, `2`, ...
  in streamvoice/avo/_aquacmn/. The folder names are truncations of the cell
  name (`_aquacmn` for AVO_AquaCmnt*), not species, so a folder groups the
  clips of one cell family. One clip is picked at random, never the one the
  last alien line used while another variant exists. The pick is seeded per
  launch.
- No clip for a line: `alien_slot` gives Silence (emotion row 28, no sound),
  no cell (the race has no such category), or a cell with no clip shipped.
  195 of the 659 non-empty cells have no clip in the install (e.g. Chandra-Fan
  angry, T3 laughter_normal). Those lines are silent.
- 24 clips in streamvoice/avo name no alienvo cell at all, so no line can
  reach them: avo_granquesl/m/s1-3 (Gran question), avo_gransadl1-3,
  avo_niktsadl1-3, avo_weeqsadl1-3, avo_hanharhapyl1-3, avo_drdutangs1-3.
- Rule in kotor2 (`alien_clips`, `Voice::play_alien`, `dialog_ui`): a line
  plays its own VO, then its Sound, then the alien clip. The alien row is the
  node's AlienRaceNode, else the owner's AlienRaceOwner, but only when the
  owner speaks: an empty speaker tag, or a tag that resolves to the owner
  (`k2-world` `alien_row`, `named_speaker`). A named speaker that does not
  resolve is not the owner and gets no owner set. The owner's own tag names
  the owner even when an earlier object shares it (`by_tag` returns the
  first match). Replies never carry audio.
- Inferred, not checked against the original engine: in alien-owner dialogs
  the VO refs are often absent from the install (Margar's 501margar001-010
  and Geeda's 501geeda_gbl012, 037-040 in 501ond). They are the lines that
  had a VO ref and no file, so they were silent before; the alien set is the
  voice the data points to for them.
- 501ond: Margar's 10 entry lines and Geeda's 5 play the alien set now.
  Hanharr's 502502han001-012 are missing too. Unverified: that they play when
  Hanharr is the conversation owner. 501ond has no Hanharr object, so they
  stay silent there.
- Coverage (`kotor2 --audio-survey`, "alien lines"): 3933 entry lines with an
  alien race across all modules. Voiced after: 3591 (own file 104 + alien
  clip 3487). Silent: 342 (Silence 260, no cell 54, cell with no clip 28).
  Decode failures: 0. This is a lower bound: the survey counts only an empty
  speaker tag as the owner. Named owners (e.g. DarthNihilus lines in 852nih)
  are only known in a running conversation, so they are not counted.
- Lip sync: the 501ond alien lines have no LIP. Their VO names are absent
  from lips/501ond_loc.mod, while existing VO names are found there (checked
  by name; other modules are not checked). `on_line` (cutscene_cam) builds the
  synthetic track for the playing clip's length, passed through `AlienLine`
  (set by `dialog_ui` in `LineShown`, which `on_line` runs after). The track
  ends up to one 0.09 s step before the clip ends (`synthetic_lip` steps
  0.09 s), and a clip shorter than 0.09 s gets a 0.09 s track.
- Tests: `audio::tests::alien_owner_lines_501ond` decodes every reachable
  clip and checks Margar's and Geeda's lines (prints SKIP without an install);
  `audio::tests::alien_variant_does_not_repeat`; `dialog::tests` (k2-world)
  for the owner rule; `tests/alien_voice.rs` for empty, owner, named-owner,
  unresolved and named-non-owner speakers on the real modules;
  `cutscene_cam::tests` for the lip length.
- Headless: `k2world talk 501ond margar` shows `alien row 1 emotion 4`;
  `k2world talk 303nar vogga` shows `alien row 13` on the Vogga lines.

## Area audio

- Music and ambient beds come from the GIT `AreaProperties` struct
  (MusicDay/Night/Battle, AmbientSndDay/Night, AmbientSndDayVol/NitVol
  0-127), not the ARE. Rows index ambientmusic.2da and ambientsound.2da,
  column `resource`. ARE `IsNight` selects the night variants.
- Music loops at 0.6 times master. KotOR waits MusicDelay between repeats;
  we loop seamlessly for now.

## Sound objects (UTS), as the shipped data uses the flags

- Looping=1: one wave looped seamlessly. Every steady hum in the game (vents,
  lights, kolto tanks, computers) is Looping=1, Continuous=0, Interval 0.
- Continuous=1: waves play one at a time, sequentially or at random
  (Random), with Interval +- IntervalVrtn ms of silence after each, forever.
  Radio chatter, beeps, steam, rumbles and alarms are Continuous=1,
  Looping=0 with multi-second intervals.
- Both 0: the list plays through once, then the object goes inactive
  (scripted one-shots, usually Active=0 until a script starts them).
- An earlier version read Continuous as "seamless loop" and played the
  interval sounds back to back forever.
- Positional=1: Bevy spatial audio for panning. rodio's own falloff is 1/d^2,
  far too steep for meters, so each clip's spatial scale is 1/MaxDistance
  (rodio stays at 1 inside the radius). `attenuate_emitters` applies a linear
  ramp from MinDistance (full volume) to MaxDistance (silent) every frame,
  and pauses clips that are out of range.
- Not done yet: RandomPosition/RandomRange, PitchVariation, VolumeVrtn,
  prioritygroups.2da, EnvAudio reverb.
- Scripts drive sound objects: SoundObjectPlay (a loop already running keeps
  running, a one-shot restarts), SoundObjectStop, SoundObjectFadeAndStop
  (volume ramp, then despawn), SoundObjectSetVolume (0..127, live) and
  SoundObjectSetFixedVariance (semitones, as playback speed 2^(v/12)). The
  emitter is found by the world object's tag, nearest by XY when several
  share it; events for an emitter that does not exist yet wait up to 3 s.
  Clip starts of script-driven objects log at info. See minigames.md.
- The listener is on the camera, with an ear gap of 0.3 m.

## API for dialog and UI

`audio::Voice` SystemParam:
- `play_voice(resref) -> Option<Duration>` stops the current line, plays the
  new one non-positionally and returns its length for auto-advance.
- `stop_voice()`.
- `play_sound(resref)` plays a one-shot effect.

Flags: `--mute`, `--volume F` (master, linear), `--audio-test MODULE` (print
the resolved area plan and decode every clip, no window), `--audio-survey`.
After 3 s the game logs how many sinks are live (101per: 4 plain, 83 spatial,
4 audible from the entry point).

## Combat and creature sounds (`crates/kotor2/src/sfx.rs`)

All from the game's tables; nothing hardcoded beyond fallback delays.

- Weapon row: baseitems.2da `weaponmattype` of the right-hand weapon, else
  the creature weapon slots (CWEAPON_R/B/L), else appearancesndset.2da
  `weapon` via appearance `soundapptype`, else weaponsounds row 0 (Unarmed).
  Ranged weapons use baseitems `ammunitiontype` into ammunitiontypes.2da
  (`shotsound0` normal shot, `shotsound1` power variant, `impactsound0/1`).
- Hit column: target armour = baseitems `armortype` of the body or
  creature-armour item (cloth/leather/armor), else appearancesndset
  `armortype` (droids: armor, beasts: leather), else leather. weaponsounds
  has `<col>0/1` for cloth, leather, armor, forcefield, metal, wood, stone,
  plus parry0/1, swingshort0-2, swinglong0-2, swingtwirl0-2, clash0/1.
- Timing comes from MDL clip events, which the K2 supermodels carry:
  `Swingshort`/`Swinglong`/`Swingtwirl` (swing), `Hit` (impact), `Clash`,
  `HitParry`/`Contact` (parry anims), `snd_footstep`, `snd_hitground`,
  `draw_weapon`. Names vary in case (droids use `hit`); compare lowercase.
  AnimPlayer collects crossed events per frame (`drain_events`).
  Droid ranged clips (b0a*) and blaster clips carry no shot event, so shots
  fire 0.25 s after the attack starts (BOLT_LAUNCH, the bolt's launch) and a
  ranged impact plays when its bolt lands (the Projectile event's launch +
  flight; 0.4 s without one); melee clips without `hit` fall back to 0.3 s
  swing / 0.45 s impact. A deflected bolt plays the target's clash sound.
- The world emits AttackStarted and its AttackHit/AttackMissed in the same
  tick, so the outcome is known when the clip's `hit` event fires.
- Misses: parry sound only when the target holds a melee weapon.
- Voices: UTC `SoundSetFile` -> soundset.2da `resref` -> SSF V1.1 (28
  strref slots: 0-5 battle cry, 12-13 pain, 15 dead, ...) -> dialog.tlk sound
  resref. Battle cry 30% on a creature's first attack after 20 s quiet; pain
  with a 2.5 s cooldown; death on Died.
- Footsteps: appearance `footsteptype` row of footstepsounds.2da, column from
  the surfacemat.2da `sound` code of the walkmesh face underfoot (DT dirt,
  GR grass, ST stone, WD wood, WT water, CP carpet, MT metal, LV leaves;
  material Puddles -> puddles), 3 variants, scaled by `footstepvolume`.
  `snd_hitground` (falls in death clips) uses appearancesndset
  falldirt/fallhard/fallmetal/fallwater.
- One-shots are spatial children of the creature root (they follow it) with
  a linear min/max ramp, and are despawned after their length + 1 s even if
  paused out of range.
- `kotor2 --sfx-survey`: 659 unique resrefs (96 from the tables, 639 filled
  slots in 64 soundsets), 12 missing in the shipped data: fs_force1-3,
  c_drdprotocol.ssf, and SSF strrefs whose TLK sound resref has no file
  (p_t3m3_low, c_drdmk2_dam, a_twifem_die1, p_disciple_dead, p_atton_slct1,
  n_commonkm_bat, n_commonf_bat, p_hand_dmin).

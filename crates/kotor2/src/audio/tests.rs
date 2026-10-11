use std::io::Write;
use super::*;

fn install() -> Option<Resources> {
    let Ok(i) = Install::discover(std::env::var("K2_INSTALL").unwrap_or_else(|_| "../..".into()))
        .or_else(|_| Install::discover("../../.."))
    else {
        let _ = writeln!(std::io::stderr(), "SKIP audio tests: no install (set K2_INSTALL)");
        return None;
    };
    Some(Resources::open(i).unwrap_or_else(|e| panic!("the install does not open: {e:?}")))
}

#[test]
fn mp3_header_math() {
    // MPEG1 layer III, 128 kbps, 44.1 kHz, no padding: 417 bytes.
    assert_eq!(
        mp3_frame(&[0xFF, 0xFB, 0x90, 0x00]),
        Some((417, 1152, 44100))
    );
    assert_eq!(mp3_frame(&[0xFF, 0xFB, 0xF0, 0x00]), None);
}

#[test]
fn real_files_sniff() {
    let Some(res) = install() else { return };
    let idx = SoundIndex::build(&res.install);
    let cases = [
        ("0013cfd003", SoundKind::Voice, "riff+mp3"),
        ("mus_atris", SoundKind::Music, "riff+mp3"),
        ("bed_001ebo", SoundKind::Music, "riff+mp3"),
        ("al_alarm_a", SoundKind::Effect, "junk470+wav"),
    ];
    for (name, kind, variant) in cases {
        let (raw, from) = idx
            .find(&res, name, kind)
            .unwrap_or_else(|| panic!("{name} not found"));
        let s = sniff(&raw).unwrap_or_else(|e| panic!("{name} ({from}): {e}"));
        assert_eq!(s.variant, variant, "{name}");
        assert!(
            s.duration > Duration::from_millis(100),
            "{name} duration {:?}",
            s.duration
        );
        assert!(decode_check(&s, 1000).unwrap() > 0, "{name} decodes");
    }
}

/// Every alien clip that some (race, emotion) reaches decodes, and there
/// are 1386 of them (the survey's count). Also checks that every variant of
/// Margar's and Geeda's owner lines in 501ond (voice files absent from the
/// install) decodes. Prints SKIP without an install.
#[test]
fn alien_owner_lines_501ond() {
    let Some(mut res) = install() else {
        let _ = writeln!(std::io::stderr(), "SKIP alien_owner_lines_501ond: no install (set K2_INSTALL)");
        return;
    };
    res.load_module("501ond").expect("501ond");
    let index = SoundIndex::build(&res.install);
    let tables = k2_data::Tables::new();

    let races = tables.get(&res, "alienvo").expect("alienvo.2da").rows();
    let emotions = tables.get(&res, "emotion").expect("emotion.2da").rows();
    let mut reach: std::collections::BTreeSet<String> = Default::default();
    for row in 0..races {
        for e in 0..emotions {
            if let AlienSlot::Clips(clips) = alien_slot(&tables, &res, &index, row as u32, e as i32)
            {
                reach.extend(clips);
            }
        }
    }
    for clip in &reach {
        let (raw, from) = index
            .find(&res, clip, SoundKind::Voice)
            .unwrap_or_else(|| panic!("{clip} listed but not found"));
        let s = sniff(&raw).unwrap_or_else(|e| panic!("{clip} from {from}: {e}"));
        assert!(decode_check(&s, 1000).unwrap() > 0, "{clip} decodes");
    }
    assert_eq!(
        reach.len(),
        1386,
        "reachable alien clips (survey: 1386 of 1410 avo clips)"
    );

    for (dlg, race, lines) in [("margar", 1, 10), ("geeda_gbl", 18, 5)] {
        let d = k2_data::Dialog::load(&res, dlg).expect(dlg);
        assert_eq!(d.alien_race_owner, race, "{dlg} owner race");
        let mut n = 0;
        for node in &d.entries {
            let vo = node.vo_resref.as_deref().unwrap_or_default();
            assert!(
                index.find(&res, vo, SoundKind::Voice).is_none(),
                "{vo} has a file"
            );
            let clips = alien_clips(&tables, &res, &index, race, node.emotion);
            assert!(
                !clips.is_empty(),
                "{dlg} {vo}: no alien clip for emotion {}",
                node.emotion
            );
            for clip in &clips {
                let (raw, from) = index.find(&res, clip, SoundKind::Voice).expect(clip);
                let s = sniff(&raw).unwrap_or_else(|e| panic!("{clip} from {from}: {e}"));
                assert!(decode_check(&s, 1000).unwrap() > 0, "{clip} decodes");
            }
            n += 1;
        }
        assert_eq!(n, lines, "{dlg} alien lines");
    }
}

/// The pick never repeats the last clip while another one is free, and a
/// single clip is always picked.
#[test]
fn alien_variant_does_not_repeat() {
    let clips: Vec<String> = ["avo_a1", "avo_a2", "avo_a3"].map(str::to_owned).into();
    let mut rng = 12345;
    let mut last: Option<String> = None;
    let mut seen = HashSet::new();
    for _ in 0..500 {
        let i = pick_variant(&clips, last.as_deref(), &mut rng);
        assert_ne!(
            last.as_deref(),
            Some(clips[i].as_str()),
            "repeat of {}",
            clips[i]
        );
        seen.insert(i);
        last = Some(clips[i].clone());
    }
    assert_eq!(seen.len(), 3);
    let one = vec!["avo_b1".to_owned()];
    assert_eq!(pick_variant(&one, Some("avo_b1"), &mut rng), 0);
}

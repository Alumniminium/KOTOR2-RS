use super::*;

/// Fixed scratch dir under the system temp dir, reused across runs.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("k2-res-test-{name}"));
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// No chitin.key: only the mod entries and override/ resolve.
fn loose_install(root: PathBuf, mod_dirs: Vec<PathBuf>) -> Resources {
    let install = Install { root, mod_dirs };
    let (overrides, mod_capsules) = mount_overrides(&install);
    Resources {
        install,
        bifs: Vec::new(),
        chitin: HashMap::new(),
        overrides,
        mod_capsules,
        texpacks: Vec::new(),
        modules: Vec::new(),
    }
}

fn payload(n: usize, seed: u8) -> Vec<u8> {
    (0..n)
        .map(|i| (i as u8).wrapping_mul(7).wrapping_add(seed))
        .collect()
}

#[test]
fn mod_capsule_round_trip() {
    let path = scratch("mod-round-trip").join("editor.mod");
    let entries = vec![
        (ResKey::new("Area01", ResourceType::Git), payload(300, 1)),
        (
            ResKey::new("c_bantha_e01", ResourceType::Utc),
            payload(5000, 2),
        ),
        (ResKey::new("empty", ResourceType::Are), Vec::new()),
    ];
    write_mod(&path, entries.clone()).unwrap();
    assert_eq!(&fs::read(&path).unwrap()[..8], b"MOD V1.0");

    let res = loose_install(path.parent().unwrap().to_owned(), vec![path]);
    for (key, data) in &entries {
        assert_eq!(
            res.get(key).unwrap().as_deref(),
            Some(data.as_slice()),
            "{key}"
        );
    }
    assert!(
        res.get(&ResKey::new("missing", ResourceType::Git))
            .unwrap()
            .is_none()
    );
}

#[test]
fn mod_capsule_rejects_long_names() {
    let path = scratch("mod-long-name").join("bad.mod");
    let key = ResKey {
        name: "x".repeat(17),
        ty: ResourceType::Utc.type_id(),
    };
    assert!(write_mod(&path, vec![(key, Vec::new())]).is_err());
}

#[test]
fn mod_entries_shadow_override_first_wins() {
    let root = scratch("mod-order");
    let ov = root.join("override");
    fs::create_dir_all(&ov).unwrap();
    fs::write(ov.join("area01.git"), b"override git").unwrap();
    fs::write(ov.join("c_only.utc"), b"override only").unwrap();
    let loose = scratch("mod-order-loose");
    fs::write(loose.join("area01.git"), b"loose git").unwrap();
    let capsule = scratch("mod-order-capsule").join("capsule.mod");
    write_mod(
        &capsule,
        vec![
            (
                ResKey::new("area01", ResourceType::Git),
                b"capsule git".to_vec(),
            ),
            (
                ResKey::new("c_only", ResourceType::Utc),
                b"capsule only".to_vec(),
            ),
            (
                ResKey::new("c_capsule", ResourceType::Utc),
                b"capsule utc".to_vec(),
            ),
        ],
    )
    .unwrap();

    let git = ResKey::new("area01", ResourceType::Git);
    let res = loose_install(root.clone(), vec![capsule.clone(), loose.clone()]);
    assert_eq!(res.get(&git).unwrap().unwrap(), b"capsule git");
    assert_eq!(
        res.get(&ResKey::new("c_only", ResourceType::Utc))
            .unwrap()
            .unwrap(),
        b"capsule only"
    );
    assert_eq!(
        res.get(&ResKey::new("c_capsule", ResourceType::Utc))
            .unwrap()
            .unwrap(),
        b"capsule utc"
    );

    let res = loose_install(root, vec![loose, capsule]);
    assert_eq!(res.get(&git).unwrap().unwrap(), b"loose git");
    assert!(matches!(res.locate(&git), Some(Origin::Override(_))));
    assert_eq!(
        res.get(&ResKey::new("c_only", ResourceType::Utc))
            .unwrap()
            .unwrap(),
        b"capsule only"
    );
}

#[test]
fn a_bad_mod_capsule_is_skipped_and_the_rest_still_mount() {
    let root = scratch("mod-bad");
    let bad = root.join("broken.mod");
    fs::write(&bad, b"not an erf capsule").unwrap();
    let good = scratch("mod-bad-good").join("good.mod");
    write_mod(
        &good,
        vec![(ResKey::new("c_ok", ResourceType::Utc), b"ok".to_vec())],
    )
    .unwrap();
    let res = loose_install(root, vec![bad, good]);
    assert_eq!(
        res.get(&ResKey::new("c_ok", ResourceType::Utc))
            .unwrap()
            .unwrap(),
        b"ok"
    );
}

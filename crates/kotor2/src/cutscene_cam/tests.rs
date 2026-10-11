use super::*;

#[test]
fn alien_line_lip_runs_as_long_as_its_clip() {
    let alien = AlienLine {
        line: Some(("margar".into(), 1)),
        seconds: 2.5,
    };
    assert_eq!(lip_seconds(&alien, "margar", 1, "You! Come here!"), 2.5);
    assert_eq!(
        lip_seconds(&alien, "margar", 2, "Another line"),
        read_seconds("Another line")
    );
    assert_eq!(
        lip_seconds(&alien, "geeda_gbl", 1, "Another dialog"),
        read_seconds("Another dialog")
    );
    assert_eq!(
        lip_seconds(&AlienLine::default(), "margar", 1, "x"),
        read_seconds("x")
    );
}

#[test]
fn synthetic_lip_ends_within_one_step_of_the_clip() {
    let lip = synthetic_lip("You! Come here!", 2.5);
    assert!(
        lip.length <= 2.5 && lip.length > 2.5 - 0.09 - 1e-4,
        "{}",
        lip.length
    );
}

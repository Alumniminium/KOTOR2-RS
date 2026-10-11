use super::*;

fn control(tag: &str, top: f32, move_to: MoveTo) -> Control {
    Control {
        kind: Kind::Button,
        tag: tag.into(),
        id: -1,
        rect: [0.0, top, 100.0, 30.0],
        move_to,
        ..Control::parse(&GffStruct::new(0))
    }
}

#[test]
fn swapped_controls_take_each_others_places_and_neighbours() {
    // Column, top to bottom: OPTIONS (0), EXIT (1), MORE (2).
    let mv = |up, down| MoveTo {
        up,
        down,
        left: -1,
        right: -1,
    };
    let mut d = GuiDef {
        name: "test".into(),
        root: control("root", 0.0, MoveTo::default()),
        controls: vec![
            control("OPTIONS", 0.0, mv(-1, 1)),
            control("EXIT", 40.0, mv(0, 2)),
            control("MORE", 80.0, mv(1, -1)),
        ],
    };
    d.swap_places(1, 2);
    // EXIT now sits at the bottom and MORE in the middle.
    assert_eq!(d.controls[1].rect[1], 80.0);
    assert_eq!(d.controls[2].rect[1], 40.0);
    assert_eq!(d.controls[0].move_to, mv(-1, 2)); // OPTIONS: DOWN is MORE
    assert_eq!(d.controls[1].move_to, mv(2, -1)); // EXIT: UP is MORE, no DOWN
    assert_eq!(d.controls[2].move_to, mv(0, 1)); // MORE: UP is OPTIONS, DOWN is EXIT
}

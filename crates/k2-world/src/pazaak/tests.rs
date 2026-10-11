use super::*;

#[test]
fn codes_and_items_round_trip() {
    for n in 1..=23 {
        assert_eq!(Card::from_item_number(n).unwrap().item_number(), n);
    }
    assert_eq!(Card::from_code("*3"), Some(Card::PlusMinus(3)));
    assert_eq!(Card::from_code("-6"), Some(Card::Minus(6)));
    assert_eq!(Card::from_code("$$"), Some(Card::Double));
    assert_eq!(Card::from_code("VV"), Some(Card::OneOrTwo));
}

#[test]
fn ai_vs_ai_matches_finish() {
    let deck = || {
        (1..=5)
            .flat_map(|n| [Card::PlusMinus(n), Card::Minus(n)])
            .collect::<Vec<_>>()
    };
    for seed in 1..200u64 {
        let mut g = Pazaak::new([deck(), deck()], seed);
        let mut steps = 0;
        loop {
            match g.phase {
                Phase::Turn => {
                    let p = g.turn;
                    assert!(g.ai_turn(p));
                }
                Phase::SetOver { .. } => g.next_set(),
                Phase::MatchOver { winner } => {
                    assert_eq!(g.sides[winner].sets_won, SETS_TO_WIN);
                    break;
                }
            }
            steps += 1;
            assert!(steps < 2000, "seed {seed} did not finish");
        }
    }
}

#[test]
fn flip_and_double() {
    let mut g = Pazaak::new([vec![Card::Flip24, Card::Double], vec![]], 7);
    g.sides[0].table = vec![
        TableCard {
            value: 4,
            side: None,
        },
        TableCard {
            value: 7,
            side: None,
        },
    ];
    g.turn = 0;
    g.played_this_turn = false;
    let slot = g.sides[0]
        .hand
        .iter()
        .position(|c| *c == Some(Card::Flip24))
        .unwrap();
    g.play(0, slot, 0).unwrap();
    assert_eq!(g.sides[0].total(), 3);
}

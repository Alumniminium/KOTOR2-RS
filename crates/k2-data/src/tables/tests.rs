use super::*;
use rakata_formats::twoda::TwoDa;

fn sample() -> Table {
    let mut t = TwoDa::new(vec!["Model".into(), "Num".into()]);
    t.push_row("0", vec!["PLC_Chair".into(), "****".into()])
        .unwrap();
    t.push_row("1", vec!["".into(), "0x10".into()]).unwrap();
    Table::new("sample", t)
}

#[test]
fn empty_cells_are_none_and_case_folds() {
    let t = sample();
    assert_eq!(t.resref(0, "model").as_deref(), Some("plc_chair"));
    assert_eq!(t.int(0, "num"), None);
    assert_eq!(t.str(1, "Model"), None);
    assert_eq!(t.int(1, "NUM"), Some(16));
    assert_eq!(t.find("model", "plc_chair"), Some(0));
}

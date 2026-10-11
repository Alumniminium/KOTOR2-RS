use super::*;

#[test]
fn unit_cost_and_yield() {
    assert_eq!(unit_cost(0), 1);
    assert_eq!(unit_cost(50), 5);
    assert_eq!(unit_cost(55), 6);
    assert_eq!(breakdown_yield(100, 0), 2);
    assert_eq!(breakdown_yield(100, 10), 5);
    assert_eq!(breakdown_yield(100, 40), 10);
    assert!(breakdown_yield(1, 0) >= 1);
}

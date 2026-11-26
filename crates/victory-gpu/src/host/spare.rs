use super::Spare;

/// Kept things come back by size, the last given first, and nothing is
/// kept past the limit.
#[test]
fn kept_things_come_back_by_size_within_the_limit() {
    let mut spare: Spare<u32> = Spare::new();
    assert_eq!(spare.take(4), None);
    assert_eq!(spare.give(1, 4, 10, 100), None);
    assert_eq!(spare.give(2, 1, 10, 200), None);
    assert_eq!(spare.give(3, 4, 10, 300), None);
    assert_eq!(spare.bytes, 9);
    // Past the limit: handed back, nothing changes.
    assert_eq!(spare.give(4, 4, 10, 400), Some(4));
    assert_eq!(spare.bytes, 9);
    assert_eq!(spare.take(2), None, "no thing of that size");
    assert_eq!(spare.take(4), Some(3));
    assert_eq!(spare.take(4), Some(1));
    assert_eq!(spare.take(4), None);
    assert_eq!(spare.take(1), Some(2));
    assert_eq!(spare.bytes, 0);
    assert!(spare.kept.is_empty());
}

#[test]
fn trim_releases_only_cold_items_past_warm_ceiling() {
    let mut spare: Spare<u32> = Spare::new();
    assert_eq!(spare.give(10, 8, 32, 1_000), None);
    assert_eq!(spare.give(20, 8, 32, 2_000), None);
    assert_eq!(spare.give(30, 8, 32, 19_000), None);
    assert_eq!(spare.bytes, 24);
    // At now = 20_000, idle_ms = 15_000, warm_bytes = 8:
    // item 10 (age 19s) and 20 (age 18s) trim; item 30 (age 1s) stays.
    let freed = spare.trim(8, 15_000, 20_000, false);
    assert_eq!(freed, vec![(10, 8), (20, 8)]);
    assert_eq!(spare.bytes, 8);
    assert_eq!(spare.take(8), Some(30));
}

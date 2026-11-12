use pager::{footer, page, page_count};

#[test]
fn counts_a_partial_last_page() {
    assert_eq!(page_count(0, 10), 0);
    assert_eq!(page_count(10, 10), 1);
    assert_eq!(page_count(11, 10), 2);
    assert_eq!(page_count(25, 10), 3);
    assert_eq!(page_count(5, 0), 0);
}

#[test]
fn pages_count_from_one() {
    let items: Vec<u32> = (1..=25).collect();
    assert_eq!(page(&items, 1, 10), (1..=10).collect::<Vec<_>>());
    assert_eq!(page(&items, 2, 10), (11..=20).collect::<Vec<_>>());
    assert_eq!(page(&items, 3, 10), vec![21, 22, 23, 24, 25]);
    assert!(page(&items, 4, 10).is_empty());
    assert!(page(&items, 0, 10).is_empty());
    assert!(page(&items, 1, 0).is_empty());
}

#[test]
fn exact_fit_has_no_empty_extra_page() {
    let items: Vec<u32> = (1..=20).collect();
    assert_eq!(page(&items, 2, 10), (11..=20).collect::<Vec<_>>());
    assert!(page(&items, 3, 10).is_empty());
    assert_eq!(footer(2, 20, 10), "Page 2 of 2");
    assert_eq!(footer(3, 25, 10), "Page 3 of 3");
}

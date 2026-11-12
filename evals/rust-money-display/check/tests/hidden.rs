use money::Money;

fn show(cents: i64) -> String {
    Money::from_cents(cents).to_string()
}

#[test]
fn small_amounts() {
    assert_eq!(show(0), "$0.00");
    assert_eq!(show(7), "$0.07");
    assert_eq!(show(50), "$0.50");
    assert_eq!(show(100), "$1.00");
}

#[test]
fn thousands_get_commas() {
    assert_eq!(show(99_999), "$999.99");
    assert_eq!(show(123_450), "$1,234.50");
    assert_eq!(show(100_000_000), "$1,000,000.00");
    assert_eq!(show(12_345_678_901), "$123,456,789.01");
}

#[test]
fn negative_amounts() {
    assert_eq!(show(-307), "-$3.07");
    assert_eq!(show(-123_450), "-$1,234.50");
    assert_eq!(show(-5), "-$0.05");
}

#[test]
fn the_extremes_do_not_overflow() {
    assert_eq!(show(i64::MIN), "-$92,233,720,368,547,758.08");
    assert_eq!(show(i64::MAX), "$92,233,720,368,547,758.07");
}

#[test]
fn works_in_format_strings() {
    let total = Money::from_cents(1999) + Money::from_cents(1);
    assert_eq!(format!("Total: {total}"), "Total: $20.00");
}

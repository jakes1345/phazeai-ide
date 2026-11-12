//! US dollar amounts.

use std::fmt;

/// An amount of US dollars, held as whole cents.
///
/// It displays as a dollar sign, the dollars with a comma between each
/// group of three digits, a dot and exactly two digits of cents:
/// `$1,234.50`, `$0.07`, `$1,000,000.00`. A negative amount has a minus
/// sign before the dollar sign: `-$3.07`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Money {
    pub cents: i64,
}

impl Money {
    pub fn from_cents(cents: i64) -> Self {
        Self { cents }
    }
}

impl std::ops::Add for Money {
    type Output = Money;
    fn add(self, other: Money) -> Money {
        Money::from_cents(self.cents + other.cents)
    }
}

impl fmt::Display for Money {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let abs = self.cents.unsigned_abs();
        let dollars = (abs / 100).to_string();
        let mut grouped = String::new();
        for (i, c) in dollars.chars().enumerate() {
            if i > 0 && (dollars.len() - i) % 3 == 0 {
                grouped.push(',');
            }
            grouped.push(c);
        }
        let sign = if self.cents < 0 { "-" } else { "" };
        write!(f, "{sign}${grouped}.{:02}", abs % 100)
    }
}

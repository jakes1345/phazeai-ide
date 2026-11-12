"""Money helpers. All amounts are whole cents."""

from decimal import ROUND_HALF_UP, Decimal

TAX_RATE = Decimal("0.0825")


def round_cents(amount: Decimal) -> int:
    """Round to whole cents, halves up."""
    return int(amount.quantize(Decimal("1"), rounding=ROUND_HALF_UP))


def add_tax(cents: int) -> int:
    return round_cents(Decimal(cents) * (1 + TAX_RATE))


def apply_coupon(cents: int, coupon: dict | None) -> int:
    """Take a coupon off ``cents``: {"percent": 10} or {"fixed": 500}.

    Never goes below zero.
    """
    if not coupon:
        return cents
    if "percent" in coupon:
        off = round_cents(Decimal(cents) * Decimal(coupon["percent"]) / 100)
    else:
        off = int(coupon["fixed"])
    return max(0, cents - off)

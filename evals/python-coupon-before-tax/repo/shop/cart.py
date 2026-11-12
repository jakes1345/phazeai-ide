"""A shopping cart."""

from .pricing import add_tax, apply_coupon


class Cart:
    def __init__(self):
        self.items = []  # (name, unit price in cents, quantity)
        self.coupon = None

    def add(self, name: str, price_cents: int, qty: int = 1) -> None:
        if qty < 1:
            raise ValueError("quantity must be at least 1")
        self.items.append((name, price_cents, qty))

    def use_coupon(self, coupon: dict) -> None:
        self.coupon = coupon

    def subtotal(self) -> int:
        return sum(price * qty for _, price, qty in self.items)

    def total(self) -> int:
        """What the customer pays, in cents."""
        taxed = add_tax(self.subtotal())
        return apply_coupon(taxed, self.coupon)

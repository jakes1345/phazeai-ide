import unittest

from shop.cart import Cart
from shop.pricing import apply_coupon


class CouponsBeforeTax(unittest.TestCase):
    def cart(self, cents):
        c = Cart()
        c.add("thing", cents)
        return c

    def test_percent_coupon_comes_off_before_tax(self):
        c = self.cart(10000)
        c.use_coupon({"percent": 10})
        # 10000 - 10% = 9000, plus 8.25% tax = 9742.5, rounded half up.
        self.assertEqual(c.total(), 9743)

    def test_fixed_coupon_comes_off_before_tax(self):
        c = self.cart(3000)
        c.use_coupon({"fixed": 500})
        # 2500 + 206.25 tax.
        self.assertEqual(c.total(), 2706)

    def test_coupon_bigger_than_the_cart(self):
        c = self.cart(300)
        c.use_coupon({"fixed": 500})
        self.assertEqual(c.total(), 0)

    def test_no_coupon_is_unchanged(self):
        self.assertEqual(self.cart(2400).total(), 2598)

    def test_apply_coupon_itself_is_unchanged(self):
        self.assertEqual(apply_coupon(1000, {"percent": 15}), 850)
        self.assertEqual(apply_coupon(100, {"fixed": 500}), 0)

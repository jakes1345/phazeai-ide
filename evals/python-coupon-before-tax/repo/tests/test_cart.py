import unittest

from shop.cart import Cart


class CartTotals(unittest.TestCase):
    def test_tax_without_coupon(self):
        cart = Cart()
        cart.add("mug", 1200, 2)
        self.assertEqual(cart.subtotal(), 2400)
        self.assertEqual(cart.total(), 2598)

    def test_quantity_must_be_positive(self):
        with self.assertRaises(ValueError):
            Cart().add("mug", 1200, 0)

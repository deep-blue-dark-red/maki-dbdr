import unittest

from shop.cart import Item, apply_discount, total


class TestCart(unittest.TestCase):
    def test_total_three_items(self):
        items = [Item(10, 2), Item(5, 1), Item(3, 4)]
        self.assertEqual(total(items), 37)

    def test_total_single_item(self):
        self.assertEqual(total([Item(9, 3)]), 27)

    def test_total_empty(self):
        self.assertEqual(total([]), 0)

    def test_discount(self):
        self.assertEqual(apply_discount(100, 15), 85.0)

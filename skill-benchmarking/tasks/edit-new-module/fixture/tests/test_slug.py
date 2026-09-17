import unittest

from util.slug import slugify


class TestSlugify(unittest.TestCase):
    def test_basic(self):
        self.assertEqual(slugify("Hello, World!"), "hello-world")

    def test_collapse_spaces(self):
        self.assertEqual(slugify("  a  b  "), "a-b")

    def test_non_ascii_is_separator(self):
        self.assertEqual(slugify("Café au lait"), "caf-au-lait")

    def test_all_junk(self):
        self.assertEqual(slugify("!!!"), "")

    def test_digits_kept(self):
        self.assertEqual(slugify("v2.0 beta"), "v2-0-beta")

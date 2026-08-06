import unittest

from lib.config import resolve_config


class TestResolveConfig(unittest.TestCase):
    def test_defaults(self):
        cfg = resolve_config()
        self.assertEqual(cfg["retries"], 3)

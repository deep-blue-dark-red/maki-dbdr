import unittest

from core.app import App
from core.settings import load_cfg


class TestSettings(unittest.TestCase):
    def test_missing_file_returns_empty(self):
        self.assertEqual(load_cfg("nope.json"), {})

    def test_app_uses_settings(self):
        app = App("nope.json")
        self.assertIsNone(app.get("theme"))

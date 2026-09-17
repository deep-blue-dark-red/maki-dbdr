import json
import os

DEFAULT_PATH = "settings.json"


def load_cfg(path=None):
    """Load settings from JSON, falling back to empty dict."""
    path = path or DEFAULT_PATH
    if not os.path.exists(path):
        return {}
    with open(path) as f:
        return json.load(f)

"""Configuration loading and resolution."""

import json
import os

DEFAULTS = {
    "timeout": 30,
    "retries": 3,
    "cache_dir": "~/.cache/app",
    "log_level": "info",
}

ENV_PREFIX = "APP_"


def _env_overrides():
    out = {}
    for key, value in os.environ.items():
        if key.startswith(ENV_PREFIX):
            out[key[len(ENV_PREFIX):].lower()] = value
    return out


def _load_file(path):
    """Read a JSON config file."""
    with open(path) as f:
        return json.load(f)


def resolve_config(path=None):
    """Merge defaults, optional config file, and environment overrides."""
    merged = dict(DEFAULTS)
    if path is not None and os.path.exists(path):
        merged.update(_load_file(path))
    merged.update(_env_overrides())
    return merged

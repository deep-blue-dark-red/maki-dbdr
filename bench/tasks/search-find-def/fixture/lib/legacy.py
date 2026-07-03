"""Deprecated shims kept for the 0.9 CLI. Uses resolve_config indirectly."""

import warnings

from lib.config import resolve_config

RESOLVE_CONFIG_DOC = "resolve_config(path) -> dict"


def get_settings(path=None):
    warnings.warn("get_settings is deprecated, call resolve_config", DeprecationWarning)
    return resolve_config(path)

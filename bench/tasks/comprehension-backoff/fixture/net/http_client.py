import time
import urllib.request

BASE_DELAY = 0.25
MAX_DELAY = 8.0
MAX_ATTEMPTS = 6


def _backoff_delay(attempt):
    """Exponential backoff: BASE_DELAY doubles each attempt, capped at MAX_DELAY."""
    return min(BASE_DELAY * (2 ** attempt), MAX_DELAY)


def get(url):
    last_err = None
    for attempt in range(MAX_ATTEMPTS):
        try:
            with urllib.request.urlopen(url, timeout=10) as resp:
                return resp.read()
        except OSError as err:
            last_err = err
            time.sleep(_backoff_delay(attempt))
    raise last_err

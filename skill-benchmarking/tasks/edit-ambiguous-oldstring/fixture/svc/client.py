import time

DELAY = 0.5


def _ping(host):
    return host == "localhost"


def _get(url):
    return None if "flaky" in url else {"url": url}


def _prime(key):
    return hash(key)


def ping_host(host):
    attempts = 0
    while attempts < 3:
        attempts += 1
        time.sleep(DELAY)
        if _ping(host):
            return True
    return False


def fetch_with_retry(url):
    attempts = 0
    while attempts < 6:
        attempts += 1
        time.sleep(DELAY)
        resp = _get(url)
        if resp is not None:
            return resp
    raise TimeoutError(url)


def warm_cache(keys):
    attempts = 0
    for key in keys:
        attempts += 1
        time.sleep(DELAY)
        _prime(key)
    return attempts

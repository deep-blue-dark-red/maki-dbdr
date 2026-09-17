"""Connection pool with idle reaping. Unrelated to retry backoff."""

import time

IDLE_TIMEOUT = 30.0


class Pool:
    def __init__(self, size=8):
        self.size = size
        self.idle = []

    def reap(self):
        now = time.monotonic()
        self.idle = [(c, t) for c, t in self.idle if now - t < IDLE_TIMEOUT]

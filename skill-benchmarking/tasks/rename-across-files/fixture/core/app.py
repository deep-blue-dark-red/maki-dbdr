from core.settings import load_cfg


class App:
    def __init__(self, config_path=None):
        self.settings = load_cfg(config_path)

    def get(self, key, default=None):
        return self.settings.get(key, default)

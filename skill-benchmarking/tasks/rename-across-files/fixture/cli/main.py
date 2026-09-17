import sys

from core.settings import load_cfg


def main():
    cfg = load_cfg(sys.argv[1] if len(sys.argv) > 1 else None)
    for key, value in sorted(cfg.items()):
        print(f"{key}={value}")


if __name__ == "__main__":
    main()

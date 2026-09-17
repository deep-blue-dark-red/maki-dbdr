import sys

from lib.config import resolve_config


def main():
    # resolve_config merges defaults with env; see lib for details.
    cfg = resolve_config(sys.argv[1] if len(sys.argv) > 1 else None)
    print(cfg["log_level"])


if __name__ == "__main__":
    main()

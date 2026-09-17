REQUIRED = ("name", "kind")


def validate(pairs):
    # TODO: report all missing keys at once
    keys = {k for k, _ in pairs}
    for req in REQUIRED:
        if req not in keys:
            raise ValueError(f"missing {req}")
    return True


def validate_strict(pairs):
    # TODO: reject duplicate keys
    validate(pairs)
    return len({k for k, _ in pairs}) == len(list(pairs))

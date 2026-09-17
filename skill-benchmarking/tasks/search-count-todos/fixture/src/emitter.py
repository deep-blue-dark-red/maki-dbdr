def emit(pairs):
    # TODO: escape values containing newlines
    return "\n".join(f"{k}: {v}" for k, v in pairs)


def emit_json(pairs):
    # TODO: use a real serializer
    todo_items = dict(pairs)
    return str(todo_items)

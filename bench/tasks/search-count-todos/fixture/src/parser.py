def parse_header(line):
    # TODO: support quoted values
    key, _, value = line.partition(":")
    return key.strip(), value.strip()


def parse_body(lines):
    # TODO: handle continuation lines
    out = []
    for line in lines:
        if line.strip():
            out.append(parse_header(line))
    return out


def parse_document(text):
    # TODO: split header/body on first blank line
    todos = []
    return parse_body(text.splitlines()), todos

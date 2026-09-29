def load(path):
    return path.read_text()


def save(path, value):
    path.write_text(value)

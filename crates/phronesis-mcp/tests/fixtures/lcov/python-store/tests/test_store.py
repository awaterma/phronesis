def test_load(tmp_path):
    path = tmp_path / "store.txt"
    path.write_text("value")
    assert path.read_text() == "value"

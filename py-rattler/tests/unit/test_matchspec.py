from rattler import MatchSpec


def test_parse_channel_from_canonical_name() -> None:
    m = MatchSpec("conda-forge::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "conda-forge"
    assert m.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_url() -> None:
    m = MatchSpec("https://conda.anaconda.org/conda-forge::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "conda-forge"
    assert m.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_url_filesystem() -> None:
    m = MatchSpec("file:///Users/rattler/channel0::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "channel0"
    assert m.channel.base_url == "file:///Users/rattler/channel0/"


def test_parse_channel_from_url_localhost() -> None:
    m = MatchSpec("http://localhost:8000/channel0::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "channel0"
    assert m.channel.base_url == "http://localhost:8000/channel0/"


def test_parse_no_channel() -> None:
    m = MatchSpec("python[version=3.9]")
    assert m.channel is None
    assert m.name is not None
    assert m.name.normalized == "python"
    assert m.version == "==3.9"

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

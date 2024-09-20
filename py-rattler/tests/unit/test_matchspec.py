from rattler import MatchSpec


def test_parse_channel_from_canonical_name():
    m = MatchSpec("conda-forge::python[version=3.9]")
    assert m.channel.name == "conda-forge"
    assert m.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_url():
    m = MatchSpec("https://conda.anaconda.org/conda-forge::python[version=3.9]")
    assert m.channel.name == "conda-forge"
    assert m.channel.base_url == "https://conda.anaconda.org/conda-forge/"

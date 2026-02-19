import pytest

from rattler import MatchSpec, NamelessMatchSpec


def test_parse_channel_from_canonical_name() -> None:
    m = MatchSpec("conda-forge::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "conda-forge"
    assert m.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_canonical_name_nameless() -> None:
    m = MatchSpec("conda-forge::python[version=3.9]")
    nms = NamelessMatchSpec.from_match_spec(m)
    assert nms.channel is not None
    assert nms.channel.name == "conda-forge"
    assert nms.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_url() -> None:
    m = MatchSpec("https://conda.anaconda.org/conda-forge::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "conda-forge"
    assert m.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_url_nameless() -> None:
    m = MatchSpec("https://conda.anaconda.org/conda-forge::python[version=3.9]")
    nms = NamelessMatchSpec.from_match_spec(m)
    assert nms.channel is not None
    assert nms.channel.name == "conda-forge"
    assert nms.channel.base_url == "https://conda.anaconda.org/conda-forge/"


def test_parse_channel_from_url_filesystem() -> None:
    m = MatchSpec("file:///Users/rattler/channel0::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "channel0"
    assert m.channel.base_url == "file:///Users/rattler/channel0/"


def test_parse_channel_from_url_filesystem_nameless() -> None:
    m = MatchSpec("file:///Users/rattler/channel0::python[version=3.9]")
    nms = NamelessMatchSpec.from_match_spec(m)
    assert nms.channel is not None
    assert nms.channel.name == "channel0"
    assert nms.channel.base_url == "file:///Users/rattler/channel0/"


def test_parse_channel_from_url_localhost() -> None:
    m = MatchSpec("http://localhost:8000/channel0::python[version=3.9]")
    assert m.channel is not None
    assert m.channel.name == "channel0"
    assert m.channel.base_url == "http://localhost:8000/channel0/"


def test_parse_channel_from_url_localhost_nameless() -> None:
    m = MatchSpec("http://localhost:8000/channel0::python[version=3.9]")
    nms = NamelessMatchSpec.from_match_spec(m)
    assert nms.channel is not None
    assert nms.channel.name == "channel0"
    assert nms.channel.base_url == "http://localhost:8000/channel0/"


def test_parse_no_channel() -> None:
    m = MatchSpec("python[version=3.9]")
    assert m.channel is None
    assert m.name is not None
    package_name = m.name.as_package_name()
    assert package_name is not None
    assert package_name.normalized == "python"
    assert m.version == "==3.9"


def test_parse_no_channel_nameless() -> None:
    m = MatchSpec("python[version=3.9]")
    nms = NamelessMatchSpec.from_match_spec(m)
    assert nms.channel is None
    assert nms.version == "==3.9"


def test_experimental_extras_enabled() -> None:
    """Test that extras syntax can be parsed when experimental_extras is enabled."""
    m = MatchSpec("numpy[extras=[test]]", experimental_extras=True)
    assert m.name is not None
    assert m.name.normalized == "numpy"
    assert m.extras == ["test"]


def test_experimental_extras_disabled() -> None:
    """Test that extras syntax fails to parse when experimental_extras is disabled."""
    with pytest.raises(Exception):  # Should raise InvalidBracketKey error
        MatchSpec("numpy[extras=[test]]", experimental_extras=False)


def test_experimental_extras_default() -> None:
    """Test that extras syntax fails to parse by default (experimental_extras defaults to False)."""
    with pytest.raises(Exception):  # Should raise InvalidBracketKey error
        MatchSpec("numpy[extras=[test]]")


def test_experimental_extras_multiple() -> None:
    """Test parsing multiple extras with experimental_extras enabled."""
    m = MatchSpec("numpy[extras=[test,dev,docs]]", experimental_extras=True)
    assert m.name is not None
    assert m.name.normalized == "numpy"
    assert m.extras == ["test", "dev", "docs"]


def test_experimental_conditionals_enabled() -> None:
    """Test that conditionals syntax can be parsed when experimental_conditionals is enabled."""
    m = MatchSpec('requests[when="python >=3.6"]', experimental_conditionals=True)
    assert m.name is not None
    assert m.name.normalized == "requests"
    assert m.condition == "python >=3.6"


def test_experimental_conditionals_disabled() -> None:
    """Test that conditionals are rejected when experimental_conditionals is disabled."""
    # When disabled, the when key should be rejected as invalid
    with pytest.raises(Exception):
        MatchSpec('requests[when="python >=3.6"]', experimental_conditionals=False)


def test_experimental_conditionals_default() -> None:
    """Test that conditionals are rejected by default (experimental_conditionals defaults to False)."""
    # When disabled, the when key should be rejected as invalid
    with pytest.raises(Exception):
        MatchSpec('requests[when="python >=3.6"]')


def test_deprecated_if_syntax_returns_error() -> None:
    """Test that the deprecated '; if' syntax returns an error."""
    # Old syntax should always return an error, regardless of experimental_conditionals setting
    with pytest.raises(Exception):
        MatchSpec("requests; if python >=3.6", experimental_conditionals=True)
    with pytest.raises(Exception):
        MatchSpec("requests; if python >=3.6", experimental_conditionals=False)


def test_experimental_both_features() -> None:
    """Test using both experimental extras and conditionals together."""
    m = MatchSpec('numpy[extras=[test], when="python >=3.7"]', experimental_extras=True, experimental_conditionals=True)
    assert m.name is not None
    assert m.name.normalized == "numpy"
    assert m.extras == ["test"]
    assert m.condition == "python >=3.7"


def test_nameless_experimental_extras() -> None:
    """Test that NamelessMatchSpec also supports experimental extras."""
    nms = NamelessMatchSpec(">=1.20[extras=[test]]", experimental_extras=True)
    assert nms.version == ">=1.20"
    assert nms.extras == ["test"]


def test_strict_mode_with_experimental_features() -> None:
    """Test that experimental features work with strict parsing mode."""
    m = MatchSpec("numpy[extras=[test]]", strict=True, experimental_extras=True)
    assert m.name is not None
    assert m.name.normalized == "numpy"
    assert m.extras == ["test"]


def test_lenient_mode_with_experimental_features() -> None:
    """Test that experimental features work with lenient parsing mode."""
    m = MatchSpec("numpy[extras=[test]]", strict=False, experimental_extras=True)
    assert m.name is not None
    assert m.name.normalized == "numpy"
    assert m.extras == ["test"]


def test_extras_and_condition_none_by_default() -> None:
    """Test that extras and condition are None when features are not enabled."""
    m = MatchSpec("numpy >=1.20")
    assert m.name is not None
    assert m.name.normalized == "numpy"
    assert m.version == ">=1.20"
    assert m.extras is None
    assert m.condition is None

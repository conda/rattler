from __future__ import annotations
from rattler.rattler import PyMirrorMiddleware, PyAuthenticationMiddleware, PyOciMiddleware, PyGCSMiddleware


class MirrorMiddleware:
    def __init__(self, mirrors: dict[str, list[str]]) -> None:
        """
        Create a new MirrorMiddleware instance.
        The mirrors argument should be a dictionary where the keys are the
        original mirror URLs and the values are lists of mirror URLs to
        replace the original mirror with.

        Examples
        --------
        ```python
        >>> from rattler.networking import Client
        >>> middleware = MirrorMiddleware({"https://conda.anaconda.org/conda-forge": ["https://repo.prefix.dev/conda-forge"]})
        >>> middleware
        MirrorMiddleware()
        >>> Client([middleware])
        Client()
        >>>
        ```
        """
        self._middleware = PyMirrorMiddleware(mirrors)

    def __repr__(self) -> str:
        """
        Returns a representation of the Middleware

        Examples
        --------
        ```python
        >>> middleware = MirrorMiddleware({"https://conda.anaconda.org/conda-forge": ["https://repo.prefix.dev/conda-forge"]})
        >>> middleware
        MirrorMiddleware()
        >>>
        ```
        """
        return f"{type(self).__name__}()"


class AuthenticationMiddleware:
    """
    Middleware to handle authentication from keychain
    """

    def __init__(self) -> None:
        self._middleware = PyAuthenticationMiddleware()

    def __repr__(self) -> str:
        """
        Returns a representation of the Middleware

        Examples
        --------
        ```python
        >>> from rattler.networking import Client
        >>> middleware = AuthenticationMiddleware()
        >>> middleware
        AuthenticationMiddleware()
        >>> Client([middleware])
        Client()
        >>>
        ```
        """
        return f"{type(self).__name__}()"


class OciMiddleware:
    """
    Middleware to handle `oci://` URLs
    """

    def __init__(self) -> None:
        self._middleware = PyOciMiddleware()

    def __repr__(self) -> str:
        """
        Returns a representation of the Middleware

        Examples
        --------
        ```python
        >>> from rattler.networking import Client
        >>> middleware = OciMiddleware()
        >>> middleware
        OciMiddleware()
        >>> Client([middleware])
        Client()
        >>>
        ```
        """
        return f"{type(self).__name__}()"


class GCSMiddleware:
    """
    Middleware to work with gcs:// URLs

    Examples
    --------
    ```python
    >>> from rattler.networking import Client
    >>> middleware = GCSMiddleware()
    >>> middleware
    GCSMiddleware()
    >>> Client([middleware])
    Client()
    """

    def __init__(self) -> None:
        self._middleware = PyGCSMiddleware()

    def __repr__(self) -> str:
        return f"{type(self).__name__}()"

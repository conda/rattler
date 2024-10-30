from __future__ import annotations
from rattler.rattler import PyMirrorMiddleware, PyAuthenticationMiddleware


class MirrorMiddleware:
    def __init__(self, middlewares: dict[str, list[str]]) -> None:
        self._middleware = PyMirrorMiddleware(middlewares)
    
    def __repr__(self) -> str:
        """
        Returns a representation of the Middleware

        Examples
        --------
        ```python
        >>> from rattler.networking import AuthenticatedClient
        >>> middleware = MirrorMiddleware({"https://conda.anaconda.org/conda-forge": ["https://repo.prefix.dev/conda-forge"]})
        >>> middleware
        MirrorMiddleware()
        >>> AuthenticatedClient([middleware])
        AuthenticatedClient()
        >>>
        ```
        """
        return f"{type(self).__name__}()"


class AuthenticationMiddleware:
    def __init__(self, abc: str) -> None:
        self._middleware = PyAuthenticationMiddleware(abc)

    def __repr__(self) -> str:
        """
        Returns a representation of the Middleware

        Examples
        --------
        ```python
        >>> from rattler.networking import AuthenticatedClient
        >>> middleware = AuthenticationMiddleware("hello")
        >>> middleware
        AuthenticationMiddleware()
        >>> AuthenticatedClient([middleware])
        AuthenticatedClient()
        >>>
        ```
        """
        return f"{type(self).__name__}()"


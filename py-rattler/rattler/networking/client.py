from __future__ import annotations
from rattler.rattler import PyClientWithMiddleware
from rattler.networking.middleware import AuthenticationMiddleware, MirrorMiddleware


class Client:
    """
    A client that can be used to make requests.
    """

    def __init__(self, middlewares: list[AuthenticationMiddleware | MirrorMiddleware] | None = None) -> None:
        self._client = PyClientWithMiddleware(
            [middleware._middleware for middleware in middlewares] if middlewares else None
        )

    @classmethod
    def _from_ffi_object(cls, client: PyClientWithMiddleware) -> Client:
        """
        Construct py-rattler Client from PyClientWithMiddleware FFI object.
        """
        client = cls.__new__(cls)
        client._client = client
        return client

    def __repr__(self) -> str:
        """
        Returns a representation of the Client

        Examples
        --------
        ```python
        >>> Client()
        Client()
        >>>
        ```
        """
        return f"{type(self).__name__}()"

    @staticmethod
    def authenticated_client() -> Client:
        """
        Returns an authenticated client.

        Examples
        --------
        ```python
        >>> Client.authenticated_client()
        Client()
        >>>
        """
        return Client([AuthenticationMiddleware()])

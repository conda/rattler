from __future__ import annotations

from rattler.networking.middleware import (
    AddHeadersMiddleware,
    AuthenticationMiddleware,
    GCSMiddleware,
    MirrorMiddleware,
    OciMiddleware,
    S3Middleware,
)
from rattler.rattler import PyClientWithMiddleware


class Client:
    """
    A client that can be used to make requests.
    """

    def __init__(
        self,
        middlewares: (
            list[
                AddHeadersMiddleware
                | AuthenticationMiddleware
                | MirrorMiddleware
                | OciMiddleware
                | GCSMiddleware
                | S3Middleware
            ]
            | None
        ) = None,
        headers: dict[str, str] | None = None,
        timeout: int | None = None,
    ) -> None:
        self._client = PyClientWithMiddleware(
            [middleware._middleware for middleware in middlewares] if middlewares else None,
            headers,
            timeout,
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
        ```
        """
        return Client([AuthenticationMiddleware()])

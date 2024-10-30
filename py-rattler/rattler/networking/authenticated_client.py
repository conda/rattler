from __future__ import annotations
from rattler.rattler import PyAuthenticatedClient
from rattler.networking.middleware import MirrorMiddleware


class AuthenticatedClient:
    """
    A client that can be used to make authenticated requests.
    """

    def __init__(self, middlewares: list[MirrorMiddleware] | None = None) -> None:
        self._client = PyAuthenticatedClient([middleware._middleware for middleware in middlewares] if middlewares else None)

    @classmethod
    def _from_ffi_object(cls, client: PyAuthenticatedClient) -> AuthenticatedClient:
        """
        Construct py-rattler AuthenticatedClient from PyAuthenticatedClient FFI object.
        """
        authenticated_client = cls.__new__(cls)
        authenticated_client._client = client
        return authenticated_client

    def __repr__(self) -> str:
        """
        Returns a representation of the AuthenticatedClient

        Examples
        --------
        ```python
        >>> AuthenticatedClient()
        AuthenticatedClient()
        >>>
        ```
        """
        return f"{type(self).__name__}()"

from __future__ import annotations
from rattler.rattler import PyAuthenticatedClient


class AuthenticatedClient:
    """
    A client that can be used to make authenticated requests.
    """

    def __init__(self) -> None:
        self._client = PyAuthenticatedClient()

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

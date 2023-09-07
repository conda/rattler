from __future__ import annotations
from typing import Self
from rattler.rattler import PyAuthenticatedClient


class AuthenticatedClient:
    """
    A client that can be used to make authenticated requests.
    """

    def __init__(self):
        self._client = PyAuthenticatedClient()

    @classmethod
    def _from_ffi_object(cls, client: PyAuthenticatedClient) -> Self:
        """
        Construct py-rattler AuthenticatedClient from PyAutheticatedClient FFI object.
        """
        authenticated_client = cls.__new__(cls)
        authenticated_client._client = client
        return authenticated_client

    def __str__(self) -> str:
        """
        Returns the string representation of the AuthenticatedClient
        """
        return ""

    def __repr__(self) -> str:
        """
        Returns a representation of the AuthenticatedClient

        Examples
        --------
        >>> AuthenticatedClient()
        AuthenticatedClient()
        """
        return f"{type(self).__name__}({self.__str__()})"

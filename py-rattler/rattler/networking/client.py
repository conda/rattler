from __future__ import annotations

from rattler.config import Config
from rattler.networking.middleware import (
    AddHeadersMiddleware,
    AuthenticationMiddleware,
    GCSMiddleware,
    MirrorMiddleware,
    OciMiddleware,
    RetryMiddleware,
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
                | RetryMiddleware
                | MirrorMiddleware
                | OciMiddleware
                | GCSMiddleware
                | S3Middleware
            ]
            | None
        ) = None,
        headers: dict[str, str] | None = None,
        user_agent: str | None = None,
        timeout: int | None = None,
    ) -> None:
        self._client = PyClientWithMiddleware(
            [middleware._middleware for middleware in middlewares] if middlewares else None,
            headers,
            user_agent,
            timeout,
        )

    @classmethod
    def _from_ffi_object(cls, client: PyClientWithMiddleware) -> Client:
        """
        Construct py-rattler Client from PyClientWithMiddleware FFI object.
        """
        instance = cls.__new__(cls)
        instance._client = client
        return instance

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
    def default_client(
        max_retries: int = 3,
        headers: dict[str, str] | None = None,
        user_agent: str | None = None,
        timeout: int | None = None,
    ) -> Client:
        """
        Returns a client with the standard middleware stack: retry,
        authentication, OCI, GCS and S3.

        Args:
            max_retries: Maximum retry attempts for transient errors (default 3).
            headers: Default headers to send with requests.
            user_agent: User-Agent header to send with requests.
            timeout: Request timeout in seconds.

        Examples
        --------
        ```python
        >>> Client.default_client()
        Client()
        >>>
        ```
        """
        return Client(
            [
                RetryMiddleware(max_retries),
                AuthenticationMiddleware(),
                OciMiddleware(),
                GCSMiddleware(),
                S3Middleware(),
            ],
            headers=headers,
            user_agent=user_agent,
            timeout=timeout,
        )

    @staticmethod
    def from_config(
        config: Config,
        max_retries: int = 3,
        headers: dict[str, str] | None = None,
        user_agent: str | None = None,
        timeout: int | None = None,
    ) -> Client:
        """Build the standard client using networking settings from ``config``.

        This applies certificate verification, HTTP/HTTPS proxies,
        ``authentication-override-file``, mirrors, and per-bucket S3 options.
        Explicit client options such as ``headers`` and ``timeout`` remain
        available to the caller.

        Examples
        --------
        ```python
        >>> from rattler import Config
        >>> client = Client.from_config(Config.from_toml("tls-no-verify = true"))
        >>> client
        Client()
        >>>
        ```
        """
        return Client._from_ffi_object(
            PyClientWithMiddleware.from_config(
                config._inner,
                max_retries,
                headers,
                user_agent,
                timeout,
            )
        )

    @staticmethod
    def authenticated_client() -> Client:
        """
        Returns an authenticated client with the full default middleware stack.

        This is equivalent to :py:meth:`default_client` and kept for
        backwards compatibility.

        Examples
        --------
        ```python
        >>> Client.authenticated_client()
        Client()
        >>>
        ```
        """
        return Client.default_client()

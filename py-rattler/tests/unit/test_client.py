# type: ignore
from __future__ import annotations

import rattler.networking.client as client_module
from rattler.networking import Client
from rattler.networking.middleware import (
    AuthenticationMiddleware,
    AzureMiddleware,
    GCSMiddleware,
    OciMiddleware,
    RetryMiddleware,
    S3Middleware,
)


def test_default_client_stack_includes_azure(monkeypatch) -> None:
    """The default client's middleware stack must include every cloud backend."""
    constructed: list[type] = []

    for name in (
        "RetryMiddleware",
        "AuthenticationMiddleware",
        "OciMiddleware",
        "GCSMiddleware",
        "AzureMiddleware",
        "S3Middleware",
    ):
        original = getattr(client_module, name)

        def record(*args, _original=original, **kwargs):
            constructed.append(_original)
            return _original(*args, **kwargs)

        monkeypatch.setattr(client_module, name, record)

    client = Client.default_client()

    assert isinstance(client, Client)
    for middleware in (
        RetryMiddleware,
        AuthenticationMiddleware,
        OciMiddleware,
        GCSMiddleware,
        AzureMiddleware,
        S3Middleware,
    ):
        assert middleware in constructed

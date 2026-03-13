from typing import Protocol, runtime_checkable


@runtime_checkable
class InstallProgressDelegate(Protocol):
    """Protocol for objects that receive install progress events.

    Implement any subset of the following methods to receive callbacks during
    the install process.  Methods that are missing on the delegate are silently
    ignored.

    If any method raises an exception, the install will fail and the exception
    will be propagated to the caller after the current batch of concurrent
    operations completes.
    """

    def on_unlink_start(self, package_name: str) -> None:
        """Called when unlinking (removing) a package begins."""
        ...

    def on_unlink_complete(self, package_name: str) -> None:
        """Called when unlinking (removing) a package completes."""
        ...

    def on_link_start(self, package_name: str) -> None:
        """Called when linking (installing) a package begins."""
        ...

    def on_link_complete(self, package_name: str) -> None:
        """Called when linking (installing) a package completes."""
        ...

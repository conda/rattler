from __future__ import annotations

from rattler.rattler import PyNoArchType


class NoArchType():

    _noarch: PyNoArchType

    @classmethod
    def _from_py_no_arch_type(cls, py_no_arch_type: PyNoArchType) -> NoArchType:
        """Construct Rattler NoArchType from FFI PyNoArchType object."""
        no_arch_type = cls.__new__(cls)
        no_arch_type._noarch = py_no_arch_type
        return no_arch_type
    
    @property
    def generic(self) -> bool:
        """
        Return whether this NoArchType is 'generic'
        >>> NoArchType('generic')
        True
        >>>
        """
        return self._noarch.generic
    
    @property
    def none(self) -> bool:
        """
        Return whether this NoArchType is set
        >>> NoArchType(None)
        True
        >>>
        """
        return self._noarch.none
    
    @property
    def python(self) -> bool:
        """
        Return whether this NoArchType is 'python'
        >>> NoArchType('python')
        True
        >>>
        """
        return self._noarch.python

    def __hash__(self) -> int:
        """
        Computes the hash of this instance.

        Examples
        --------
        ```python
        >>> hash(NoArchType("python")) == hash(NoArchType("python"))
        True
        >>> hash(NoArchType("python")) == hash(NoArchType("abc-test"))
        False
        >>>
        ```
        """
        return self._type.__hash__()

    def __eq__(self, other: object) -> bool:
        """
        Returns True if this instance represents the same NoArchType as `other`.

        Examples
        --------
        ```python
        >>> NoArchType("python") == NoArchType("generic")
        False
        >>> NoArchType("python") == NoArchType("python")
        True
        >>> NoArchType("generic") == NoArchType("generic")
        True
        >>> NoArchType("python") == "python"
        False
        >>>
        ```
        """
        if not isinstance(other, NoArchType):
            return False

        return self._type == other._type

    def __ne__(self, other: object) -> bool:
        """
        Returns True if this instance does not represents the same NoArchType as `other`.

        Examples
        --------
        ```python
        >>> NoArchType("python") != NoArchType("python")
        False
        >>> NoArchType("python") != "python"
        True
        >>>
        ```
        """
        if not isinstance(other, NoArchType):
            return True

        return self._type != other._type

    def __repr__(self) -> str:
        """
        Returns a representation of the NoArchType.

        Examples
        --------
        ```python
        >>> p = NoArchType("python")
        >>> p
        NoArchType("python")
        >>>
        ```
        """
        return f'NoArchType("{self._noarch}")'

# BuildString

`PackageRecord.build`, `IndexJson.build`, and `GenericVirtualPackage.build_string`
return `BuildString` objects. Use `str(build)` when you need a Python string.
These getters preserve legacy values without validation, so builds can be copied
between records without revalidating them.

Constructors and setters accept either a validated plain string or a `BuildString`.
Use `BuildString.new_unchecked(value)` to explicitly bypass CEP26 validation.

::: rattler.package.build_string

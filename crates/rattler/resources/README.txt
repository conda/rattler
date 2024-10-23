# The Python launchers

To launch Python entrypoints on Windows, we need a corresponding executable (shebangs do not work on Windows).

For this reason, we use the same `cli-32.exe` and `cli-64.exe` files as found in `conda`.
These entry points are signed by Anaconda Inc.

They were taken at the following commit: be5da40dc6f18de7d0690afa1fefff13955ef4ab from the conda git repository ()

The hashes are:

- `cli-32.exe`: `0170dda609519c088b1e4619a1e1d15a01701a6c514bb55f99a85fcbbd541631`
    - source in conda/conda: https://github.com/conda/conda/blob/be5da40dc6f18de7d0690afa1fefff13955ef4ab/conda/shell/cli-32.exe
- `cli-64.exe`: `92c15cccdeecc3856c69dd7d47fe01d8d7782b6f4ada1dbd86790a5e702ea73f`
    - source in conda/conda: https://github.com/conda/conda/blob/be5da40dc6f18de7d0690afa1fefff13955ef4ab/conda/shell/cli-64.exe

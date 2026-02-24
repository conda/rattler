import re
import os
import sys


def extract_python_docstring_raises(filepath):
    """Extracts exceptions from the 'Raises:' section of Python docstrings."""
    with open(filepath, "r") as f:
        content = f.read()

    # Simple regex to find function/method and its docstring
    # This is a bit naive but should work for the current codebase
    matches = re.finditer(r'async def (\w+)\(.*?\) -> .*?:\n\s+"""(.*?)"""', content, re.DOTALL)

    results = {}
    for match in matches:
        func_name = match.group(1)
        docstring = match.group(2)

        raises_section = re.search(r'Raises:\n(.*?)(?:\n\n|\n\s*\n|"""|$)', docstring, re.DOTALL)
        if raises_section:
            exceptions = re.findall(r"\s+(\w+):", raises_section.group(1))
            results[func_name] = set(exceptions)

    # Also check non-async functions
    matches = re.finditer(r'def (\w+)\(.*?\) -> .*?:\n\s+"""(.*?)"""', content, re.DOTALL)
    for match in matches:
        func_name = match.group(1)
        if func_name in results:
            continue
        docstring = match.group(2)

        raises_section = re.search(r'Raises:\n(.*?)(?:\n\n|\n\s*\n|"""|$)', docstring, re.DOTALL)
        if raises_section:
            exceptions = re.findall(r"\s+(\w+):", raises_section.group(1))
            results[func_name] = set(exceptions)

    return results


def extract_rust_errors(filepath):
    """Heuristically extracts possible errors from Rust binding functions."""
    with open(filepath, "r") as f:
        content = f.read()

    # Look for pyfunction or pymethods blocks
    # This is also heuristic
    # We look for functions that return PyResult
    matches = re.finditer(r"pub fn (py_\w+)\(.*?\)\s*->\s*PyResult<.*?>\s*\{(.*?)\}", content, re.DOTALL)

    # Map of PyRattlerError variants to Python exception names
    error_map = {
        "SolverError": "SolverError",
        "IoError": "IoError",
        "TransactionError": "TransactionError",
        "GatewayError": "GatewayError",
        "ExtractionError": "ExtractError",  # Note: extracted as PyRattlerError::ExtractError
        "ExtractError": "ExtractError",
        "FetchRepoDataError": "FetchRepoDataError",
        "InvalidVersion": "InvalidVersionError",
        "InvalidVersionSpec": "InvalidVersionSpecError",
        "InvalidMatchSpec": "InvalidMatchSpecError",
        "InvalidUrl": "InvalidUrlError",
        "InvalidChannel": "InvalidChannelError",
        "ConvertSubdirError": "ConvertSubdirError",
        "ParsePlatformError": "ParsePlatformError",
        "ParseArchError": "ParseArchError",
    }

    results = {}
    for match in matches:
        func_name = match.group(1)
        body = match.group(2)

        # Look for map_err(PyRattlerError::from) or specific error returns
        found_exceptions = set()
        if "PyRattlerError" in body:
            # We assume most things can raise SolverError, IoError etc if they use this crate
            # But let's look for specific ones if possible via grep-like patterns
            for variant, py_name in error_map.items():
                if variant in body:
                    found_exceptions.add(py_name)

        if found_exceptions:
            results[func_name] = found_exceptions

    return results


def verify():
    base_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    py_dir = os.path.join(base_dir, "rattler")
    rust_dir = os.path.join(base_dir, "src")

    print(f"Verifying docstrings in {py_dir} against Rust implementation in {rust_dir}...")

    # Define mapping from Python files to Rust files
    mapping = {
        os.path.join(py_dir, "solver/solver.py"): os.path.join(rust_dir, "solver.rs"),
        os.path.join(py_dir, "install/installer.py"): os.path.join(rust_dir, "installer.rs"),
        os.path.join(py_dir, "index/index.py"): os.path.join(rust_dir, "index.rs"),
    }

    # Python function name to Rust binding function name mapping
    func_mapping = {
        "solve": "py_solve",
        "solve_with_sparse_repodata": "py_solve_with_sparse_repodata",
        "install": "py_install",
        "index": "py_index_fs",  # Simplified
    }

    success = True
    for py_file, rust_file in mapping.items():
        if not os.path.exists(py_file) or not os.path.exists(rust_file):
            continue

        py_raises = extract_python_docstring_raises(py_file)
        rust_raises = extract_rust_errors(rust_file)

        for py_func, docs in py_raises.items():
            rust_func = func_mapping.get(py_func)
            if rust_func and rust_func in rust_raises:
                expected = rust_raises[rust_func]
                # Check if all expected exceptions are documented
                missing = expected - docs
                if missing:
                    print(f"Error in {py_file}:{py_func}: Missing documented exceptions: {missing}")
                    success = False

                # Check if documented matches expected (roughly)
                # Note: some exceptions might be raised implicitly or are common
                extra = docs - expected
                if extra:
                    # We allow extra documentation for now if it's common ones
                    pass

    if success:
        print("Verification successful!")
    else:
        print("Verification failed!")
        sys.exit(1)


if __name__ == "__main__":
    verify()

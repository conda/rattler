<a href="https://prefix.dev/tools/rattler/">
  <img src="https://github.com/user-attachments/assets/73dee0d8-b372-4462-bce1-f004c5f907b5" alt="banner">
</a>

# Rattler: Rust crates for fast handling of conda packages

![License][license-badge]
[![Build Status][build-badge]][build]
[![Project Chat][chat-badge]][chat-url]
[![Pixi Badge][pixi-badge]][pixi-url]
[![docs main][docs-main-badge]][docs-main]
[![python docs main][py-docs-main-badge]][py-docs-main]

[license-badge]: https://img.shields.io/badge/license-BSD--3--Clause-blue?style=flat-square
[build-badge]: https://img.shields.io/github/actions/workflow/status/conda/rattler/rust-compile.yml?style=flat-square&branch=main
[build]: https://github.com/conda/rattler/actions
[chat-badge]: https://img.shields.io/discord/1082332781146800168.svg?label=&logo=discord&logoColor=ffffff&color=7389D8&labelColor=6A7EC2&style=flat-square
[chat-url]: https://discord.gg/kKV8ZxyzY4
[docs-main-badge]: https://img.shields.io/badge/rust_docs-main-yellow.svg?style=flat-square
[docs-main]: https://conda.github.io/rattler
[py-docs-main-badge]: https://img.shields.io/badge/python_docs-main-yellow.svg?style=flat-square
[py-docs-main]: https://conda.github.io/rattler/py-rattler
[pixi-badge]:https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json&style=flat-square
[pixi-url]: https://pixi.sh

Rattler is a library that provides common functionality used within the conda ecosystem ([what is conda & conda-forge?](#what-is-conda--conda-forge)).
The goal of the library is to enable programs and other libraries to easily interact with the conda ecosystem without being dependent on Python.
Its primary use case is as a library that you can use to provide conda related workflows in your own tools.

Rattler is written in Rust and tries to provide a clean API to its functionalities (see: [Components](#components)).
With the primary goal in mind we aim to provide bindings to different languages to make it easy to integrate Rattler in non-rust projects.

Rattler is actively used by [pixi](https://github.com/prefix-dev/pixi), [rattler-build](https://github.com/prefix-dev/rattler-build), and the https://prefix.dev backend.

## Showcase

This repository also contains a binary (use `cargo run` to try) that shows some of the capabilities of the library.
This is an example of installing an environment containing `cowpy` and all its dependencies _from scratch_ (including Python!):

![Installing an environment](https://github.com/conda/rattler/assets/4995967/c7946f6e-28a9-41ef-8836-ef4b4c94d273)

## Python and Javascript bindings

You can invoke `rattler` from Python or Javascript via our powerful bindings to solve, install and run commands in conda environments. Rattler offers you the fastest and cleanest Python bindings to the conda ecosystem.

### Python

To install the Python bindings, you can use pip or conda:

```bash
pip install py-rattler
# or
conda install -c conda-forge py-rattler
```

You can find the extensive documentation for the Python bindings [here](https://conda.github.io/rattler/py-rattler/).

<details>
  <summary>Example usage of rattler from Python</summary>
The Python bindings to rattler are designed to be used with `asyncio`. You can access the raw power of the rattler library to solve environments, install packages, and run commands in the installed environments.

```python
import asyncio
import tempfile

from rattler import solve, install, VirtualPackage

async def main() -> None:
    # Start by solving the environment.
    #
    # Solving is the process of going from specifications of package and their
    # version requirements to a list of concrete packages.
    print("started solving the environment")
    solved_records = await solve(
        # Channels to use for solving
        channels=["conda-forge"],
        # The specs to solve for
        specs=["python ~=3.12.0", "pip", "requests 2.31.0"],
        # Virtual packages define the specifications of the environment
        virtual_packages=VirtualPackage.detect(),
    )
    print("solved required dependencies")

    # Install the packages into a new environment (or updates it if it already
    # existed).
    env_path = tempfile.mkdtemp()
    await install(
        records=solved_records,
        target_prefix=env_path,
    )

    print(f"created environment: {env_path}")


if __name__ == "__main__":
    asyncio.run(main())
```
</details>

### Javascript

To use the Javascript bindings, you can install the `@conda-org/rattler` package via npm. rattler is compiled to WebAssembly and can be used in the browser or in Node.js.

```bash
npm install @conda-org/rattler
```

Using rattler from Javascript is useful to get access to the same version comparison functions as used throughout the conda ecosystem. It is also used as part of [`mambajs`](https://github.com/emscripten-forge/mambajs) which uses the rattler library to solve and install packages from the emscripten-forge channel _in the browser_.


## Give it a try!

Before you begin, make sure you have the following prerequisites:
- A recent version of [git](https://git-scm.com/book/en/v2/Getting-Started-Installing-Git)
- A recent version of [pixi](https://github.com/prefix-dev/pixi)

Follow these steps to clone, compile, and run the rattler project:
```shell
# Clone the rattler repository along with its submodules:
git clone --recursive https://github.com/conda/rattler.git
cd rattler

# Compile and execute rattler to create a JupyterLab instance:
pixi run rattler create jupyterlab
```

The above command will execute the `rattler` executable in release mode.
It will download and install an environment into the `.prefix` folder that contains [`jupyterlab`](https://jupyterlab.readthedocs.io/en/stable/getting_started/overview.html) and all the dependencies required to run it (like `python`)

Run the following command to start jupyterlab:

```shell
# on windows
.\.prefix\Scripts\jupyter-lab.exe

# on linux or macOS
 ./.prefix/bin/jupyter-lab
```

Voila!
You have a working installation of jupyterlab installed on your system!
You can of course install any package you want this way.
Try it!

## Contributing 😍

We would love to have you contribute!
See the CONTRIBUTION.md for more info. For questions, requests or a casual chat, we are very active on our discord server.
You can [join our discord server via this link][chat-url].


## Components

See [ARCHITECTURE.md](./ARCHITECTURE.md)

## What is conda & conda-forge?

The conda ecosystem provides **cross-platform**, **binary** packages that you can use with **any programming language**.
`conda` is an open-source package management system and environment management system that can install and manage multiple versions of software packages and their dependencies.
`conda` is written in Python.
The aim of Rattler is to provide all functionality required to work with the conda ecosystem from Rust.
Rattler is not a reimplementation of `conda`.
`conda` is a package management tool.
Rattler is a _library_ to work with the conda ecosystem from different languages and applications.
For example, it powers the backend of https://prefix.dev.

`conda-forge` is a community-driven effort to bring new and existing software into the conda ecosystem.
It provides _tens-of-thousands of up-to-date_ packages that are maintained by a community of contributors.
For an overview of available packages see https://prefix.dev.

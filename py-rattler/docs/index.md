<a href="https://github.com/mamba-org/rattler/">
    <picture>
      <source srcset="https://github.com/mamba-org/rattler/assets/4995967/8f5a9786-f75c-4b55-8043-69c551b22459" type="image/webp">
      <source srcset="https://github.com/mamba-org/rattler/assets/4995967/7bb44c97-e77a-452f-9a00-431b7c89e136" type="image/png">
      <img src="https://github.com/mamba-org/rattler/assets/4995967/7bb44c97-e77a-452f-9a00-431b7c89e136" alt="banner">
    </picture>
</a>

## What is this?

Rattler is a library that provides common functionality used within the conda ecosystem ([what is conda & conda-forge?](#what-is-conda--conda-forge)).
The goal of the library is to enable programs and other libraries to easily interact with the conda ecosystem without being dependent on Python.
Its primary use case is as a library that you can use to provide conda related workflows in your own tools.

Rattler is written in Rust and tries to provide a clean API to its functionalities.
With the primary goal in mind we aim to provide bindings to different languages to make it easy to integrate Rattler in non-rust projects.
Py-rattler is the python bindings for rattler.

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

## How should I use the documentation?

If you are getting started with the library, you should follow the 'First Steps' section in order.
You can also use the menu on the left to quickly skip over sections and search for specific things.

## Installation

Py-Rattler is a python library, which means you need to download and install Python from https://www.python.org/downloads
if you haven't already. Alternatively, you can use can get via conda.

### Pypi

```shell
$ python3 -m pip install --upgrade py-rattler
```

### Pixi

```shell
$ pixi add py-rattler
```

### Conda

```shell
$ conda install py-rattler
```

### Mamba

```shell
$ mamba install py-rattler -c conda-forge
```

## Quick-Start

Let's see an example to learn some of the functionality the library has to offer.

```python

import asyncio

from rattler import fetch_repo_data, solve, link, Channel, Platform, MatchSpec, VirtualPackage

def download_callback(done, total):
    print("", end = "\r")
    print(f'{done/1024/1024:.2f}MiB/{total/1024/1024:.2f}MiB', end = "\r")
    if done == total:
        print()

async def main():
    # channel to use to get the dependencies
    channel = Channel("conda-forge")

    # list of dependencies to install in the env
    match_specs = [
        MatchSpec("python ~=3.12.*"),
        MatchSpec("pip"),
        MatchSpec("requests 2.31.0")
    ]

    # list of platforms to get the repo data
    platforms = [Platform.current(), Platform("noarch")]

    virtual_packages = [p.into_generic() for p in VirtualPackage.current()]

    cache_path = "/tmp/py-rattler-cache/"
    env_path = "/tmp/env-path/env"

    print("started fetching repo_data")
    repo_data = await fetch_repo_data(
        channels = [channel],
        platforms = platforms,
        cache_path = f"{cache_path}/repodata",
        callback = download_callback,
    )
    print("finished fetching repo_data")

    solved_dependencies = solve(
        specs = match_specs,
        available_packages = repo_data,
        virtual_packages = virtual_packages,
    )
    print("solved required dependencies")

    await link(
        dependencies = solved_dependencies,
        target_prefix = env_path,
        cache_dir = f"{cache_path}/pkgs",
    )
    print(f"created environment: {env_path}")

if __name__ == "__main__":
    asyncio.run(main())

```

Py-rattler provides friendly high level functions to download
dependencies and create environments. This is done through the
`fetch_repo_data`, `solve` and `link` functions.

- `fetch_repo_data` as the name implies, fetches repo data from conda registries.
- `solve` function solves the requirements to get all the packages
  which would be required to create the environment.
- `link` function takes a list of solved dependencies to create an
  environment.

## Next Steps

These basic first steps should have gotten you started with the library.

By now, you should know how to download repo data, solve dependencies and create an
environment.

Next, we will see a quick reference summary of all the methods and properties that you will need when using the library. If you follow the links there, you will expand the documentation for the method and property, with more examples on how to use them.

## Contributing 😍

We would love to have you contribute!
See the CONTRIBUTION.md for more info. For questions, requests or a casual chat, we are very active on our [discord server](https://discord.gg/kKV8ZxyzY4).

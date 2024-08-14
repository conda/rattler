<a href="https://github.com/conda/rattler/">
    <picture>
      <source srcset="https://github.com/conda/rattler/assets/4995967/8f5a9786-f75c-4b55-8043-69c551b22459" type="image/webp">
      <source srcset="https://github.com/conda/rattler/assets/4995967/7bb44c97-e77a-452f-9a00-431b7c89e136" type="image/png">
      <img src="https://github.com/conda/rattler/assets/4995967/7bb44c97-e77a-452f-9a00-431b7c89e136" alt="banner">
    </picture>
</a>

## What is this?

Rattler is a library that provides common functionality used within the conda ecosystem ([what is conda & conda-forge?](#what-is-conda--conda-forge)).
The goal of the library is to enable programs and other libraries to easily interact with the conda ecosystem without being dependent on Python.
Its primary use case is as a library that you can use to provide conda related workflows in your own tools.

Rattler is written in Rust and tries to provide a clean API to its functionalities.
With the primary goal in mind we aim to provide bindings to different languages to make it easy to integrate Rattler in non-rust projects.
Py-rattler is the python bindings for rattler.

## Quick-Start

Let's see an example to learn some of the functionality the library has to offer.

```python
--8<-- "examples/solve_and_install.py"
```

Py-rattler provides friendly high level functions to download dependencies and create environments.
The `solve` and `install` functions are excellent examples of such high-level functions.

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

## Next Steps

These basic first steps should have gotten you started with the library.

By now, you should know how to download repo data, solve dependencies and create an
environment.

Next, we will see a quick reference summary of all the methods and properties that you will need when using the library. If you follow the links there, you will expand the documentation for the method and property, with more examples on how to use them.

## Contributing 😍

We would love to have you contribute!
See the CONTRIBUTION.md for more info. For questions, requests or a casual chat, we are very active on our [discord server](https://discord.gg/kKV8ZxyzY4).

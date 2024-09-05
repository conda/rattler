<a href="https://github.com/conda/rattler/">
    <picture>
      <source srcset="https://github.com/user-attachments/assets/6f3f05bc-6363-4974-9517-fe5c0fcffd1a" type="image/jpeg">
      <source srcset="https://github.com/user-attachments/assets/dc30403d-6392-460a-b923-986c2164ef79" type="image/webp">
      <source srcset="https://github.com/user-attachments/assets/bfd64756-061d-49f5-af4e-388743bdb855" type="image/png">
      <img src="https://github.com/user-attachments/assets/bfd64756-061d-49f5-af4e-388743bdb855" alt="banner">
    </picture>
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
[pixi-badge]:https://img.shields.io/endpoint?url=https://raw.githubusercontent.com/prefix-dev/pixi/main/assets/badge/v0.json
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

Rattler consists of several crates that provide different functionalities. 

* **rattler_conda_types**: foundational types for all datastructures used within the conda eco-system.
* **rattler_package_streaming**: provides functionality to download, extract and create conda package archives.  
* **rattler_repodata_gateway**: downloads, reads and processes information about existing conda packages from an index.
* **rattler_shell**: code to activate an existing environment and run programs in it.
* **rattler_solve**: a backend agnostic library to solve the package satisfiability problem.
* **rattler_virtual_packages**: a crate to detect system capabilities.
* **rattler_index**: create local conda channels from local packages.
* **rattler**: functionality to create complete environments from scratch using the crates above.
* **rattler-lock**: a library to create and parse lockfiles for conda environments.
* **rattler-networking**: common functionality for networking, like authentication, mirroring and more.
* **rattler-bin**: an example of a package manager using all the crates above (see: [showcase](#showcase))

You can find these crates in the `crates` folder.

Additionally, we provide Python bindings for most of the functionalities provided by the above crates.
A python package `py-rattler` is available on [conda-forge](https://prefix.dev/channels/conda-forge/packages/py-rattler) and [PyPI](https://pypi.org/project/py-rattler/).
Documentation for the python bindings can be found [here](https://conda.github.io/rattler/py-rattler).

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

## What is js-rattler?

Rattler is a library that provides common functionality used within the conda ecosystem ([what is conda & conda-forge?](#what-is-conda-conda-forge)).
The goal of the library is to enable programs and other libraries to easily interact with the conda ecosystem without being dependent on Python.
Its primary use case is as a library that you can use to provide conda related workflows in your own tools.

Rattler is written in Rust and tries to provide a clean API to its functionalities.
With the primary goal in mind we aim to provide bindings to different languages to make it easy to integrate Rattler in non-rust projects.
js-rattler is the JavaScript/TypeScript bindings for rattler, compiled to WebAssembly.

## Quick-Start

Let's see an example to learn some of the functionality the library has to offer.

```ts
--8<-- "examples/quick-start.mjs"
```

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

If you are getting started with the library, you should follow the 'Getting Started' section in order.
You can also use the menu on the left to quickly skip over sections and search for specific things.

## Installation

See [Installation](getting-started/installation.md) for how to add js-rattler to your project.

## Next Steps

These basic first steps should have gotten you started with the library.

Next, see the [Reference](reference/index.md) section for a complete overview of all the classes and functions js-rattler exposes.

## Contributing 😍

We would love to have you contribute!
See the [CONTRIBUTION.md](https://github.com/conda/rattler/blob/main/CONTRIBUTING.md) for more info. For questions, requests or a casual chat, we are very active on our [discord server](https://discord.gg/kKV8ZxyzY4).

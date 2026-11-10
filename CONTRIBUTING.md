# Contributing 😍

We would love to have you contribute!
For a good list of things you could help us with, take a look at our [*good first issues*](https://github.com/conda/rattler/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22).
Before starting work, agree on the proposed change with a maintainer on an issue or on Discord.

For questions, requests or a casual chat, we are very active on our discord server.
You can [join our discord server via this link](https://discord.gg/kKV8ZxyzY4).

## Pull Request Prerequisite: Vouching

We use a vouch system to mitigate spam. Before opening a pull request:

1. [Open a discussion in the "Vouch Request" category](https://github.com/conda/rattler/discussions/new?category=vouch-request). Briefly introduce yourself and describe what you want to change and why. Write this yourself, without AI.
2. A maintainer will comment `vouch` if approved.

For an existing pull request, a maintainer can instead comment `vouch` on the pull request to vouch for its author.

If you are already vouched for another project using the shared registry, such as [Pixi](https://github.com/prefix-dev/pixi), you don't need to request it again.
Pull requests from unvouched contributors are automatically closed.
A vouch is not approval of a specific change: agree on it with a maintainer before opening a pull request.

## Development
If you'd like to contribute code, then you may want to manage the build depends with a tool, we suggest pixi, but conda/mamba will also work.

### Virtual env with pixi
You can use [pixi](https://github.com/prefix-dev/pixi) for setting up the environment needed for building and testing rattler, (as a fun fact, pixi uses rattler as a dependency!). The spec in `pixi.toml` in the project root will set up the environment. After installing, run the install command from the project root directory, shown below.
```sh
❱ git submodule update --init
❱ pixi install # installs dependencies into the virtual env
❱ pixi run build # calls "build" task specified in pixi.toml, "cargo build", using cargo in pixi venv
```

### Virtual env with conda/mamba
The environment can also be managed with conda using the spec in `environments.yml` in the project root.
As below,
```sh
❱ git submodule update --init
❱ mamba create -n name_of_your_rattler_env --file='environments.yml' && mamba activate name_of_your_rattler_env
❱ cargo build # uses cargo from your mamba venv
❱ mamba deactivate # don't forget you're in the venv
```
### Format bindings
```sh
cd py-rattler
pixi run fmt
```

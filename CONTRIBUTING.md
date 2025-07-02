# Contributing 😍

We would love to have you contribute!
For a good list of things you could help us with, take a look at our [*good first issues*](https://github.com/conda/rattler/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22).
If you want to go deeper though, any [open issue](https://github.com/conda/rattler/issues) is up for grabs.
Just let us know what you start on something.

For questions, requests or a casual chat, we are very active on our discord server.
You can [join our discord server via this link][chat-url].

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

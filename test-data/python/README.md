This directory contains several environment file specifications that install an environment with Python.

To update the different lock files run the following command:

```shell
conda-lock lock -k explicit --filename-template "explicit-env-{platform}.txt" --micromamba -f environment.yml
```

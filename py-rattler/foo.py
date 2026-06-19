from rattler.package_streaming import fetch_raw_package_file_from_url
from rattler import Client
import asyncio

import logging


FORMAT = "%(levelname)s %(name)s %(asctime)-15s %(filename)s:%(lineno)d %(message)s"
logging.basicConfig(format=FORMAT)
logging.getLogger().setLevel(logging.DEBUG)


async def main():
    foo = await fetch_raw_package_file_from_url(
        Client(),
        "https://prefix.dev/conda-forge/noarch/polars-1.41.2-pyh58ad624_0.conda",
        "site-packages/polars/__init__.py",
    )
    print(foo[:20])


asyncio.run(main())

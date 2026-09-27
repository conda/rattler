# Installation

js-rattler is published to npm as [`@conda-org/rattler`](https://www.npmjs.com/package/@conda-org/rattler).

=== "npm"

    ```shell
    $ npm install @conda-org/rattler
    ```

=== "pnpm"

    ```shell
    $ pnpm add @conda-org/rattler
    ```

=== "yarn"

    ```shell
    $ yarn add @conda-org/rattler
    ```

The package ships as WebAssembly with both an ESM build (for bundlers and browsers) and a CommonJS build (for Node.js), so it works with either `import` or `require`:

```ts
import { Gateway, Version } from "@conda-org/rattler";
```

```js
const { Gateway, Version } = require("@conda-org/rattler");
```

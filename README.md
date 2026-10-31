# doup

A dependency updater bot inspired by:

- <https://github.com/dependabot/dependabot-core>
- <https://github.com/renovatebot/renovate>

But built using Rust with a WASM/WASI based plugin system.

## Whats with the name?

- Do Up, as in doing up a house, ala: Renovate.
- `doup` is currently available at <https://crates.io/search?q=doup> & <https://www.npmjs.com/search?q=doup>
- `doup` is also super short to type and say.

## Current Status

Right now this one of Brad's many side projects and is very much in the early stages of development.

The rust workspace currently includes the core CLI and several WASM plugins for basic arithmetic operations (add,
subtract, multiply, divide) just so Brad can get an understanding for how to structure and manage WASM plugins within
the project.

The obvious intention is to eventually completely replace this with a full-fledged dependency updater bot.

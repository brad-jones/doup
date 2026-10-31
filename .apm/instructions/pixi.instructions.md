---
description: How to work with this repo's development environment.
applyTo: "**"
---

### Pixi Development Environment

This repo is built on [pixi](https://pixi.prefix.dev/latest/). All tooling is managed via `pixi.toml`. Always prefix
commands with `pixi run` (eg: `pixi run -- bun foo.ts`) to ensure the correct environment is used.

#### Tools that don't need pixi

Tools that would typically be installed on a developers machine do not need to be prefixed with `pixi run`. eg: `git`,
`grep`, `sed`, `awk`, `curl`, `wget`, etc. If unsure check what is installed by reading `pixi.toml` at the root of the
repo.

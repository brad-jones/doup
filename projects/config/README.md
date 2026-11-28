# doup-config

Loads, validates and saves doup's two config files. The rest of the workspace reads config through this crate and never
parses the files itself.

| Type                 | Module | File names                                    | Env prefix  | Schema                                               |
| -------------------- | ------ | --------------------------------------------- | ----------- | ---------------------------------------------------- |
| `DoupBotConfigFile`  | `bot`  | `doupbot.json`, `doupbot.yaml`, `doupbot.yml` | `DOUP_BOT`  | [`bot.schema.json`](../../schemas/bot.schema.json)   |
| `DoupRepoConfigFile` | `repo` | `doup.json`, `doup.yaml`, `doup.yml`          | `DOUP_REPO` | [`repo.schema.json`](../../schemas/repo.schema.json) |

- The **bot config** sets up the bot itself: which repos to update, how to authenticate to each, which Git backend to
  use and how much work to do at once.
- The **repo config** sits at the root of each repo being updated: which platform plugin to open pull requests with, and
  which ecosystem plugins to update dependencies with.
- The **lock file** (`doup.lock`, module `lock`) sits next to the repo config & pins every package plugin to an exact
  version & content digest, see [below](#the-lock-file).

## Usage

Both types implement the `ConfigFile` trait, so they load the same way:

```rust
use doup_config::{bot::DoupBotConfigFile, file::ConfigFile, repo::DoupRepoConfigFile};

// Find doupbot.{json,yaml,yml} in the current dir or a parent, then apply DOUP_BOT_* env vars over it.
let bot = DoupBotConfigFile::load(None, None)?.merged;

for (index, repo) in bot.repos.iter().enumerate() {
  // ...clone repo.origin into `dir`...

  // Find doup.{json,yaml,yml} in `dir` or a parent up to the repo root, then apply DOUP_REPO_* and DOUP_REPO_<index>_*
  // env vars over it.
  let prefix = DoupRepoConfigFile::repo_env_prefix(index);
  let config = DoupRepoConfigFile::load(Some(dir), Some(&prefix))?.merged;
}
```

`load` returns a `Loaded<T>` with two copies of the config:

- `merged`: the file with env overrides applied. Use this to read config. It has no source path, so `save()` fails with
  `ConfigError::NoSource`, and env overrides (which may hold secrets) never end up in the file.
- `file`: exactly what is in the file, or `None` when no file was found. Use this to change and `save()` the config,
  back to the path it was loaded from.

With no file, `merged` is built from the env alone. Either way it is validated against the schema as a whole.

The rest of `ConfigFile` does each step on its own:

| Method                              | Does                                                                                         |
| ----------------------------------- | -------------------------------------------------------------------------------------------- |
| `locate_file(start_dir)`            | Walks up from `start_dir` (default: current dir) to the first of `DEFAULT_NAMES`, see below. |
| `from_file(path)` / `to_file(path)` | Reads or writes JSON or YAML, chosen by the extension (`.json`, `.yaml`, `.yml`).            |
| `from_json` / `from_yaml`           | Parses, validates and hydrates a string.                                                     |
| `to_json` / `to_yaml`               | Serializes back to a string.                                                                 |
| `from_env(env_prefix)`              | Builds a config from env vars alone.                                                         |
| `save()`                            | `to_file` back to where `from_file` read it from.                                            |

Every config round trips: `$schema` is kept if the file has one, and an SSH key given as a `key_path` is written back as
the path, not the key.

### Finding the file

`locate_file` looks in `start_dir`, then each parent in turn, and returns the first file it finds. The nearest dir wins,
and within a dir `.json` beats `.yaml`, which beats `.yml`. The repo config (`STOP_AT_GIT_ROOT`) searches no higher than
the first dir containing `.git`, so a repo without a config never picks one up from outside it, eg: from `/tmp` above a
clone. The bot config searches all the way up.

### Validation

Every way of building a config goes through `ConfigFile::validated`:

1. The raw JSON value is validated against the type's JSON schema, which is compiled into the crate at build time with
   `#[jsonschema::validator]`, so `schemas/` is not needed at runtime.
2. It is deserialized with serde.
3. `hydrate(base_dir)` runs, for checks the schema can't express. `base_dir` is the config file's dir, if there is one,
   for resolving relative paths. For the bot config this reads every `key_path` into `key` and checks each SSH key
   parses as an OpenSSH private key and is not passphrase protected.

## The bot config

```yaml
$schema: https://raw.githubusercontent.com/brad-jones/doup/refs/heads/master/schemas/bot.schema.json
git_backend: git_cli # or grit; optional, defaults to git_cli
concurrency: # optional, as is every limit in it
  repos: 4 # repos updated at once
  plugin_calls: 8 # plugin calls (version lookups, release notes, pull request changes) in flight at once, per repo
  apply_updates: 2 # dependency updates applied at once across every repo, these may run containers
repos:
  # No auth: uses the existing Git and SSH setup.
  - origin: https://github.com/owner/repo.git

  # An https:// or http:// origin takes a username and password (eg: a token).
  - origin: https://github.com/owner/repo.git
    auth:
      user: x-access-token
      pass: ghp_...

  # Any other origin (ssh:// or user@host:path) takes a private key, inline or as a path.
  - origin: git@github.com:owner/repo.git
    auth:
      key: |
        -----BEGIN OPENSSH PRIVATE KEY-----
        ...
        -----END OPENSSH PRIVATE KEY-----
  - origin: ssh://git@github.com/owner/repo.git
    auth:
      key_path: /path/to/id_ed25519
```

- `repos` must have at least one repo.
- `auth` must match the origin: `user` + `pass` for HTTP(S), and exactly one of `key` or `key_path` otherwise.
- SSH keys must be OpenSSH private keys (`-----BEGIN OPENSSH PRIVATE KEY-----`) without a passphrase.
- A relative `key_path` (from the file or the env) is looked for next to the config file first, then in the current dir.
  With no config file, or from `from_json`/`from_yaml`/`from_env`, only the current dir is used. A key found in neither
  fails with `ConfigError::SshKeyNotFound`, listing where it looked.
- After loading, the `key` of a `DoupBotRepoAuth::Ssh` is always `Some`, with `key_path` also set, exactly as written,
  if that is where it came from.
- Every `concurrency` limit must be 1 or more. As env values are strings, a string of digits is also accepted, eg:
  `DOUP_BOT_CONCURRENCY__REPOS=1`.

## The repo config

```yaml
$schema: https://raw.githubusercontent.com/brad-jones/doup/refs/heads/master/schemas/repo.schema.json
platform:
  use: doup:platform-plugin-github@0.1.0
  with:
    any: plugin specific config
ecosystems:
  - use: doup:ecosystem-plugin-cargo@0.1.0
  - use: acme:ecosystem-plugin-npm@^1
    registry: ghcr.io/acme
  - use: ./plugins/npm.wasm
    with:
      any: plugin specific config
```

Exactly one `platform` and at least one ecosystem must be given. Each plugin is a `DoupRepoPlugin`:

- `use` is a wkg package reference (`namespace:package` with an optional `@version`), or a path to a `.wasm` module.
  Anything ending in `.wasm` is a path. `DoupRepoPlugin::reference()` parses it into a `PluginRef`: a module path, or a
  `PackageSpec` whose version requirement has cargo's semantics (`@0.1.0` means `^0.1.0`, no version means any).
- `registry` optionally overrides where a package is downloaded from, & is rejected alongside a `.wasm` path:
  `host/prefix` is an OCI registry (`ghcr.io/acme` → `ghcr.io/acme/<namespace>/<package>:<version>`), a bare `host` a
  wasm-pkg registry whose metadata is discovered. Without it, the `doup` namespace maps to `ghcr.io/brad-jones` & any
  other namespace to the user's wkg config.
- `with` is an optional, untyped object handed to the plugin as JSON. The plugin validates it itself.
- `DoupRepoPlugin::name()` is a short, version independent name: the module's file stem (`./a/npm.wasm` → `npm`) or the
  package without its version (`doup:cargo@^1` → `doup:cargo`). It identifies the plugin's update branches, so switching
  a plugin from a path to a package (or back) makes doup treat it as a different plugin.

## The lock file

`DoupLockFile` is `doup.lock`, written by `doup lock` next to the repo config (`DoupRepoConfigFile::lock_path()`) &
meant to be committed. `doup update` only ever runs the exact package versions it pins, failing a repo whose package
plugin isn't in it. Its TOML mirrors wkg's `wkg.lock` (v1), plus each package's `size`, which lets downloads show their
progress:

```toml
# This file is automatically generated by `doup lock`.
# It is not intended for manual editing.
version = 1

[[packages]]
name = "doup:platform-plugin-github"

[[packages.versions]]
requirement = "^0.1.0"
version = "0.1.0"
digest = "sha256:91ab6f3239a5b525b7d3ddca670f4cdabc0863ad1258041845774ee1b35c7425"
size = 1416040
```

- An entry matches a plugin by its package name, its `registry` (as written) & its exact version requirement, so editing
  either in the config makes the entry stale.
- Packages are kept sorted, so an unchanged lock serializes identically, & it's written atomically.
- `load` returns `None` for a missing file & a `version` other than `1` fails with
  `ConfigError::UnsupportedLockVersion`.

## Environment variables

Any value can be set or overridden with an env var named `<ENV_PREFIX>_<PATH>`. `<PATH>` is the value's path in the
config, upper-cased, with `__` between its segments and array indexes as numbers, the same as .NET's configuration:

```sh
DOUP_BOT_GIT_BACKEND=grit                                    # git_backend
DOUP_BOT_REPOS__0__ORIGIN=https://github.com/owner/repo.git  # repos[0].origin
DOUP_BOT_REPOS__0__AUTH__PASS=ghp_...                        # repos[0].auth.pass
DOUP_BOT_REPOS__0__AUTH__KEY_PATH=./id_ed25519               # repos[0].auth.key_path
DOUP_REPO_PLATFORM__USE=doup:github@^1                       # platform.use
DOUP_REPO_ECOSYSTEMS__0__WITH__GITHUB_TOKEN=ghp_...          # ecosystems[0].with.github_token
```

- A single `_` is always part of a key, at any depth. Only `__` goes a level deeper.
- Env values are merged over the file: objects by key and arrays by index, so a var can override one field of an
  existing item or add a new item.
- A segment that is empty, or starts or ends with `_`, fails with `ConfigError::InvalidEnvVar`, eg: `DOUP_BOT__X`,
  `DOUP_BOT_X___Y` or `DOUP_BOT_X__`. The error names the var but never includes its value.
- Values are always strings and keys are always lowercased, so keys with capitals can't be set from the env. Numeric
  settings (eg: `concurrency.repos`) accept a string of digits for this reason.
- The schemas reject keys they don't know (other than in plugin `with` config), so a typo, or any other var that shares
  the prefix (eg: `DOUP_BOT_LOG`), fails validation rather than being ignored.

### Per-repo overrides

`load` and `from_env` take an optional `env_prefix` that is layered over `ENV_PREFIX`. doup passes
`DoupRepoConfigFile::repo_env_prefix(index)` (eg: `DOUP_REPO_0`) when loading the repo config of the bot's
`repos[index]`, so one repo can be configured differently from the rest:

```sh
DOUP_REPO_PLATFORM__USE=doup:gitlab@^2     # every repo
DOUP_REPO_0_PLATFORM__USE=doup:github@^1   # just repos[0], wins over the above
```

Like the prefix, the scope is followed by a single `_`. Vars that look like a scope (`DOUP_REPO_<n>_...`) are only ever
read as one, never as part of the `DOUP_REPO` layer, as no top-level key is a number.

## Errors

Everything fails with a `ConfigError`. Its message only describes its own step, eg: `invalid config file 'doup.yaml'`,
with the cause (a parse error, the schema violation, ...) available via `std::error::Error::source`. Render it with
`{:#}` (anyhow) or walk the chain to see it all.

## Adding a field

1. Add it to the schema in [`schemas/`](../../schemas/), which is the source of truth for editors and for validation.
2. Add the matching field to the Rust type, keeping its shape the same as the schema's.
3. If it needs checks the schema can't express, or to load something it refers to, do that in `hydrate()`.
4. Add tests, including that it round trips.

## Testing

```sh
task test:unit # in-crate tests
```

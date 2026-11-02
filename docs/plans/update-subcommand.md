# Implementation Plan: `doup update`

Status: **Ready for implementation**. All open questions are resolved; see [Decisions](#decisions).

## Goal

Make `doup update` feature complete. Run it inside a git checkout, for example in a CI job. It should:

1. Load `doup.yaml` / `doup.json`.
2. Resolve, download, verify and lock the platform and ecosystem plugins.
3. Ask each ecosystem plugin which dependencies exist and which ones are out of date.
4. For each outdated dependency, apply the update in an isolated git worktree, then commit and push a branch.
5. Ask the platform plugin to create, update or close pull requests so the repo matches the desired state.

`--dry-run` stops after step 3 and prints a table.

### In scope

- WIT contracts for **ecosystem** and **platform** plugins (replacing the calculator toy).
- Host runtime: wasmtime, WASI p2, wasi:http, logging, timeouts and memory limits.
- Config discovery, parsing and normalisation. The schema changes are listed below.
- Plugin resolution from wkg registries and local files, a cache, and `doup.lock`.
- Git orchestration: worktrees, commits, force-with-lease pushes, and stale branch cleanup.
- PR reconciliation: create, update in place, close when no longer needed.
- Fixture plugins and integration tests.

### Out of scope

- The `init` and `self-update` subcommands.
- Real platform plugins (GitHub, …) and real ecosystem plugins (npm, …).
- Grouping, filtering, rate limiting, schedules.
- Configurable branch, commit and PR templates. Defaults are hardcoded for now.
- Per-plugin network allow-lists. Network access is unrestricted in v1; an allow-list is noted as future work.
- Parallel execution. v1 runs sequentially.
- Publishing the `doup:plugin` WIT package to a registry.

---

## Corrections & Rust/WASM notes on the original sketch

These are the places where the rough notes in [update.rs](../../projects/cli/src/commands/update.rs) would run into
trouble.

### 1. Don't pass `working_dir` as a string to the plugin

A WASI guest has no access to the host filesystem. It can only see directories the host _preopens_ for it, and only
under a guest path the host picks. A host path like `/home/brad/repo` means nothing inside the sandbox.

Instead, the host preopens the target directory at a fixed guest path, **`/workspace`**, when it builds the `WasiCtx`
for that call. Plugins always read `/workspace/package.json` and so on. This also lets us apply **least privilege**:

| Call                                                 | `/workspace` permissions |
| ---------------------------------------------------- | ------------------------ |
| `list-dependencies`, `next-version`, `release-notes` | read-only                |
| `apply-update`                                       | read-write (worktree)    |
| platform calls                                       | **no filesystem at all** |

### 2. "Untyped bag of config" → JSON string

WIT has no `any` or dynamic type. Every value crossing the boundary must have a concrete WIT type. The usual pattern is
to pass plugin config as a **JSON-encoded string**. Each plugin then deserialises it into its own
`#[derive(Deserialize)] struct Config` with `serde_json`. The host never needs to understand it.

### 3. Versions are opaque strings to the host

Semver, PEP 440, calver, Go pseudo-versions and git SHAs all have different rules, which is why versioning lives inside
the ecosystem plugin. So the host **never parses** dependency versions. It only displays them and compares them for
equality.

### 4. `generate_update_diff` → `apply-update`

The plugin should just **mutate files** in `/workspace`. The host already has git, so it computes the diff with `git
status` / `git diff`. That keeps plugins simpler and avoids each plugin producing its own patch format.

### 5. Rebasing: don't rebase, rebuild

The host should not rebase existing doup branches. On every run it should **recreate the branch from the current base**
(`origin/<base>`), re-apply the update, and compare the resulting _tree_ with the remote branch. If they differ, or the
parent is stale, it force-pushes with `--force-with-lease`. This never produces merge conflicts, it is deterministic,
and it is how Renovate behaves in practice.

### 6. Go async from the start

- `wasm-pkg-client` (the library behind `wkg`) is async and uses tokio.
- `wasmtime-wasi-http` is built on tokio and hyper.
- Calling wasmtime's _sync_ WASI APIs from inside a tokio runtime can panic ("cannot start a runtime from within a
  runtime").

So use `#[tokio::main]` and run wasmtime in **async mode** (`Config::async_support(true)` and async bindgen). In Rust,
async is "contagious": an async function can only be awaited from another async function. That is why it's easier to
commit to async now than to retrofit it later.

### 7. Wasmtime object lifetimes

[plugin_host.rs](../../projects/cli/src/plugin_host.rs) currently builds everything per call. That's fine for the toy,
but wasteful for real use:

| Object             | Cost            | Lifetime                                                                                                                   |
| ------------------ | --------------- | -------------------------------------------------------------------------------------------------------------------------- |
| `Engine`           | expensive       | one per process                                                                                                            |
| `Component`        | expensive (JIT) | one per plugin, compiled once                                                                                              |
| `Linker`           | cheap-ish       | one per world (ecosystem / platform)                                                                                       |
| `Store` + instance | cheap           | one per _phase_: reused for all read-only calls, and a fresh one per `apply-update` because each uses a different worktree |

If a plugin panics, that is a _trap_, and the instance is unusable afterwards. Drop that `Store` and create a fresh one.

### 8. Worktree cleanup via RAII

Wrap each temporary worktree in a struct that implements `Drop` and runs `git worktree remove --force`. Then cleanup
happens even when an error makes us return early with `?`. `Drop` can't be async, so use `std::process::Command`
(blocking) inside it. That is fine for a quick git call.

### 9. Put the logic in a `doup` library crate

Integration tests in `tests/` can only import from a _library_ crate, and the logic shouldn't depend on clap. Create a
new `projects/doup` library crate that holds the entire pipeline. `projects/cli` becomes a thin clap wrapper around it
(see [Crate layout](#crate-layout)).

### 10. Normalise typify types immediately

typify generates types that mirror the _schema's shape_: `oneOf` becomes an enum, and a single-key map becomes a
`HashMap`. Those are awkward to pass around. Convert them into our own domain types right after loading, then never
touch the typify types again.

### 11. Keep decision logic pure

Write "given these discovered updates and these existing PRs, what should we do?" as a **pure function** that returns a
`Vec<Action>`. All the edge cases can then be unit tested without wasm, git or a network.

### 12. `serde_yaml` is deprecated → use `serde-saphyr`

The `serde_yaml` crate was archived by its author in 2024. Replace it with
[`serde-saphyr`](https://github.com/bourumir-wyngs/serde-saphyr) (decided):

- Depend on it as `serde-saphyr = "2"` with default features (`deserialize`, `serialize`, `comments`). `update` only
  reads YAML, but `init` will need to write it. `Commented<T>` can then emit explanatory comments into the generated
  config.
- Load config with `serde_saphyr::from_str_with_options`, wrapped in a single helper function so all options are set in
  one place.
- Set **`strict_booleans: true`**. Plugin `config` is deserialised untyped, into `serde_json::Value`, so no Rust type
  tells the parser what a value should be. Without this option, YAML 1.1 spellings such as `no` and `on` become
  booleans; this is the "Norway problem" (`country: no` → `false`).
- It reports errors with source snippets. Surface those as-is.
- It rejects duplicate keys by default. Keep that.
- An empty `config:` is YAML null, not `{}`. Normalise null to `{}` before passing config to a plugin.
- Parse `.json` files with `serde_json` rather than as YAML, so JSON keeps exact JSON semantics.

---

## WIT contracts

Replace `projects/wit/plugin.wit` with a `doup:plugin@0.1.0` package split across files in `projects/wit/`.

Worlds do **not** need to declare WASI imports. The host linker provides all of WASI p2 plus wasi:http, and a component
only imports what it actually uses. Guest-side HTTP crates such as `wstd` bring their own `wasi:http` bindings.

```wit
// types.wit
package doup:plugin@0.1.0;

interface types {
  /// Plugin specific configuration from the doup config file, JSON encoded.
  type config = string;

  variant plugin-error {
    /// The plugin's configuration is invalid.
    invalid-config(string),
    /// A remote service (registry, platform API, ...) failed or was unreachable.
    remote(string),
    /// Anything else.
    other(string),
  }
}

// log.wit
interface log {
  enum level { trace, debug, info, warn, error }
  log: func(level: level, message: string);
}

// ecosystem.wit
interface ecosystem {
  use types.{config, plugin-error};

  record occurrence {
    /// Manifest path relative to the workspace root, eg: "apps/web/package.json".
    manifest-path: string,
    /// Version as currently declared, eg: "^1.2.3". Opaque to the host.
    current-version: string,
  }

  record dependency {
    /// Unique within this ecosystem, eg: "@types/node".
    name: string,
    occurrences: list<occurrence>,
  }

  record release-notes {
    body: option<string>,
    url: option<string>,
  }

  /// Reads manifests under /workspace (read-only).
  list-dependencies: func(config: config) -> result<list<dependency>, plugin-error>;

  /// The version to update to, or none if up to date.
  next-version: func(config: config, dependency: dependency) -> result<option<string>, plugin-error>;

  /// Notes covering every occurrence's current version up to `to`.
  release-notes: func(config: config, dependency: dependency, to: string) -> result<option<release-notes>, plugin-error>;

  /// Edits files under /workspace (read-write worktree) to update every occurrence to `to`.
  /// Must not rely on native tooling; it can only read/write files and make HTTP requests.
  apply-update: func(config: config, dependency: dependency, to: string) -> result<_, plugin-error>;
}

// platform.wit
interface platform {
  use types.{config, plugin-error};

  record repository {
    /// URL of the remote doup pushes to.
    remote-url: string,
    /// Branch pull requests target, eg: "main".
    base-branch: string,
  }

  enum pull-request-state { open, closed, merged }

  record pull-request {
    /// Platform specific identifier, eg: the PR number on GitHub.
    id: string,
    head-branch: string,
    title: string,
    body: string,
    state: pull-request-state,
    url: string,
  }

  record pull-request-content {
    head-branch: string,
    title: string,
    body: string,
  }

  /// All open PRs whose head branch starts with `branch-prefix`, plus the most recent closed/merged PR per head branch.
  list-pull-requests: func(config: config, repo: repository, branch-prefix: string) -> result<list<pull-request>, plugin-error>;
  create-pull-request: func(config: config, repo: repository, content: pull-request-content) -> result<pull-request, plugin-error>;
  update-pull-request: func(config: config, repo: repository, id: string, content: pull-request-content) -> result<pull-request, plugin-error>;
  close-pull-request: func(config: config, repo: repository, id: string, comment: option<string>) -> result<_, plugin-error>;
}

// worlds.wit
world ecosystem-plugin {
  import log;
  export ecosystem;
}

world platform-plugin {
  import log;
  export platform;
}
```

Design notes:

- **Git push stays in the host.** It's identical across platforms and uses ambient git credentials, such as those left
  by a CI checkout. The platform plugin only talks to the PR API.
- **Labels, reviewers and assignees** belong in the platform plugin's own config, not in the contract.
- **Host bindgen:** there are two `bindgen!` calls, one per world. Use bindgen's `with:` option so the second reuses the
  `types` and `log` modules generated by the first. Otherwise you'd get two incompatible Rust copies of `PluginError`.

---

## Config changes (`doup.schema.json`)

### Plugin entry shape

The current single-key-map form (`"ref": { ...config }`) leaves nowhere to put host-level settings such as the env
allow-list without mixing them into plugin config. It also produces awkward typify output. New shape (decided, Q1):

```yaml
platform:
  plugin: "doup:github@^1"
  env: [GITHUB_TOKEN] # host env vars passed into the plugin's WASI env
  config: # any YAML/JSON value; the host re-serialises it to a JSON string when calling the plugin
    labels: [dependencies]

ecosystems:
  - "doup:npm@^1" # short form still allowed
  - plugin: "file:./target/wasm32-wasip2/debug/my_plugin.wasm"
    config: { foo: bar }

baseBranch: main # optional, defaults to the remote's default branch
```

`platform` and each `ecosystems` item use the **same** `$defs/plugin` definition: either a string (short form) or a `{
plugin, env?, config? }` object (long form). So this is equally valid:

```yaml
platform: "doup:github@^1"
```

The keyed form (`- "doup:npm@^1": { config: ... }`) was considered and rejected in favour of the `plugin:` form. The
`plugin:` form gives editors fixed keys to autocomplete, produces a simple typify struct, and gives clearer errors.

### Plugin references

The current `pluginRef` pattern is wrong in two ways. It was written with OCI paths in mind, so it allows `/path`
segments, but wkg package names are exactly `namespace:name` (`/` in WIT selects an interface, as in `wasi:http/types`).
It also rejects semver range operators. Replace it outright:

```json
"pluginRef": {
  "description": "A wkg package (namespace:name[@semver-req]) or a local component (file:<path>).",
  "type": "string",
  "anyOf": [
    { "pattern": "^[a-z][a-z0-9]*(-[a-z][a-z0-9]*)*:[a-z][a-z0-9]*(-[a-z][a-z0-9]*)*(@.+)?$" },
    { "pattern": "^file:.+$" }
  ]
}
```

- **Registry:** `namespace:name[@<semver-req>]`, for example `doup:npm@^1.2`. Each label is WIT kebab-case. The schema
  only checks the overall shape. The host then parses the name with `PackageRef::from_str` and the version with
  `VersionReq::parse`, so a bad range gets a precise error. Omitting `@...` means "latest", which is then pinned by
  `doup.lock`.
- **Local:** `file:<path>`, resolved relative to the config file. Used for plugin development and tests. Local plugins
  are never written to the lockfile.
- The OCI path (`ghcr.io/brad-jones/doup/npm`) is derived from the package name by the registry mapping. It never
  appears in config.

### Discovery

1. Use `--config <path>` if given.
2. Otherwise search `doup.yaml`, `doup.yml`, `doup.json` from the current directory up to the git root.
3. If nothing is found, return an error that suggests `doup init`.

JSONC and TOML are deferred.

---

## Plugin resolution, cache & `doup.lock`

- **Client:** use `wasm-pkg-client`.
- **Registry mapping (decided, Q2):** official plugins are published to GHCR and linked to the `brad-jones/doup` repo.
  Under the wkg OCI convention, `doup:npm@1.2.3` resolves to `ghcr.io/brad-jones/doup/npm:1.2.3`.
  - The host builds its registry `Config` in two layers. First a built-in mapping: `doup` → `RegistryMapping::Custom`
    with OCI registry `ghcr.io` and `namespacePrefix = "brad-jones/"`. Then it merges `Config::global_defaults()` on
    top, so a user's wkg `config.toml` can override it (mirrors, air-gapped setups).
  - Third-party namespaces resolve through the user's wkg config or the namespace's `/.well-known` metadata.
  - GHCR packages are private when first pushed. Each plugin package must be made public so anonymous pulls work.
  - Publishing plugins (CI workflow, `wkg publish`) is out of scope; it arrives with the first real plugin.
- **Cache (decided): use wkg's cache; don't build our own.** Wrap the `Client` in
  `wasm_pkg_client::caching::CachingClient` backed by `FileCache::global_cache()`. That cache lives in `<user cache
  dir>/wasm-pkg` and is shared with the `wkg` CLI, so components downloaded by either tool are reused.
  - `FileCache` stores component bytes by content digest and release metadata (version → digest) as
    `<package>-<version>.json`. `get_release` and `get_content` check the cache first and only go to the network on a
    miss.
  - `list_all_versions` is never cached and always hits the registry. That's fine, because doup only calls it when a
    lock entry is missing or no longer satisfies the ref.
  - **Gap we must cover:** `FileCache::put_data` streams straight to the final path without checking the digest or
    writing atomically. An interrupted download can therefore leave a truncated file that later reads return as a cache
    hit. doup already verifies sha256 against `doup.lock` before compiling (see resolution rules). On a mismatch,
    re-download through `CachingClient::client()` (which bypasses the cache), overwrite the entry with
    `FileCache::put_data`, and verify again. If it still doesn't match, fail.
  - Not adopted: a separate doup-owned store. It would duplicate what `FileCache` already does. The only thing it would
    add is atomic writes, and the digest check above covers that.
  - Future: `CachingClient::new(None, cache)` gives an offline, cache-only mode, which could back an `--offline` flag.
- **Lockfile:** `doup.lock` sits next to the config file. It uses TOML, following the Cargo.lock / wkg.lock convention.

  ```toml
  version = 1

  [[plugin]]
  ref = "doup:npm@^1"
  package = "doup:npm"
  version = "1.4.2"
  digest = "sha256:ab12..."
  ```

- **Resolution rules:**
  1. If a lock entry exists and still satisfies the ref, use the locked version.
  2. Otherwise resolve the highest matching version and update the lock.
  3. Always verify the sha256 of the bytes against the lock before compiling. Also check that the cached release
     metadata's digest matches the lock. A mismatch means the registry tag was re-pointed, which is a hard error.
- **Flags:**
  - `--locked` fails if the lock would change. Intended for CI.
  - `--dry-run` never writes the lock.
- Updating the plugins themselves is left to a future `doup:doup` ecosystem plugin (already hinted at in `doup.yaml`).

---

## Host runtime

- One `Engine` with these settings:
  - `async_support(true)`
  - `epoch_interruption(true)`
  - a background task that increments the epoch every 100 ms
- `HostState` holds:
  - `WasiCtx`
  - `ResourceTable`
  - `WasiHttpCtx`
  - `StoreLimits`
  - the plugin name, used for log attribution
- Per-call setup:
  - **Preopens:** `/workspace` only, with the permissions from the table above.
  - **Env:** only the variables in the plugin's `env` allow-list that are actually set.
  - **stdout:** routed to the host's **stderr**, so plugins can never corrupt doup's own output.
  - **Limits:** a memory cap through `StoreLimits`, `epoch_deadline_async_yield_and_update` so CPU-bound guests yield,
    and every call wrapped in `tokio::time::timeout`. In async mode, dropping the future cancels the call.
- The `log` import forwards to `tracing` with a `plugin = <name>` field.
- Typed wrappers, `EcosystemPlugin` and `PlatformPlugin`, hide the wasmtime details from the orchestration code. For
  example: `async fn list_dependencies(&self, workspace: &Path) -> Result<Vec<Dependency>, InvokeError>`.

---

## Git layer

Shell out to the `git` CLI through `tokio::process::Command` rather than using `git2` or `gix`:

- It uses the user's or CI's existing credentials and config.
- It has full worktree and push support.
- It is easy to debug.

Keep it a thin wrapper in `git.rs`. Operations needed:

- `rev-parse --show-toplevel`
- `fetch origin <base>`
- remote default branch (`symbolic-ref refs/remotes/origin/HEAD`)
- `rev-parse origin/<base>` (pin the base SHA)
- `worktree add --detach <tmp> <base-sha>` / `worktree remove --force`
- `status --porcelain`, `add -A`, `commit -m`
- `rev-parse <ref>^{tree}`, `rev-parse <ref>^`
- `push --force-with-lease=<branch>:<sha> origin HEAD:refs/heads/<branch>`
- `push origin --delete <branch>`

Commits use git's configured identity. If none is configured, fail early with a clear error.

---

## The update pipeline

```mermaid
flowchart TD
  A[Load config + lock] --> B[Resolve/fetch/verify/compile plugins]
  B --> C[Preflight: fetch base, pin base SHA, discovery worktree]
  C --> D[Discover deps per ecosystem]
  D --> E[next-version per dep]
  E -->|--dry-run| R[Print table, exit]
  E --> F[list-pull-requests prefix doup/]
  F --> G[reconcile: pure fn -> actions]
  G --> H[Per update: worktree, apply, commit, compare, push]
  H --> I[Create / update PR with release notes]
  G --> J[Close stale PRs + delete branches]
  I --> K[Summary + exit code]
  J --> K
```

### Phase details

1. **Preflight (non-dry-run only; decided, Q4)**
   - `git fetch origin <base>`, then resolve `origin/<base>` to a commit SHA once. Every worktree in this run is built
     from that pinned SHA, so discovery and updates always see the same code even if the base moves mid-run.
   - Create a detached **discovery worktree** at the pinned SHA, held by a `WorktreeGuard`.
   - The user's checkout is never read or modified. Doup behaves the same on any branch, with or without uncommitted
     changes.
   - Dry runs skip all of this (no fetch, no worktrees) and read the current checkout, so you can try out local manifest
     changes.
2. **Discovery**
   - Call `list-dependencies` on each ecosystem, with the discovery worktree (or the current checkout for dry runs)
     preopened read-only.
   - Record each ecosystem's outcome as `Ok(deps)` or `Err(e)`.
3. **Next versions**
   - Call `next-version` for each dependency.
   - The result for each one is: up to date, `PlannedUpdate { ecosystem, dependency, to }`, or an error.
4. **Dry run**
   - Print a table: ecosystem, dependency, current version(s), next version.
   - Errors are listed underneath.
   - Exit.
5. **Reconcile** with `fn reconcile(discovery, existing_prs) -> Vec<Action>`. The possible actions are:
   - `Upsert { update, existing_pr: Option<Pr> }`
   - `Close { pr, reason }`: only for branches belonging to ecosystems whose discovery **succeeded**, where the
     dependency was confirmed up to date or no longer exists. **Never close PRs for an ecosystem that errored.**
   - `Skip`: for example, the dependency's `next-version` errored, or the user declined this exact update (below).
   - **Declined updates (decided, Q3):** if a branch has no open PR, and its most recent PR was **closed without
     merging** and targeted the same `(ecosystem, dependency, to)`, skip it. When a newer `to` appears, upsert as
     normal; the branch is rebuilt and a new PR is opened. Reopening the closed PR also brings it back into the normal
     flow.
6. **Upsert**
   1. `WorktreeGuard::new(<pinned base SHA>)`.
   2. `apply-update` with the worktree preopened read-write.
   3. If there are no changes, warn and skip.
   4. Commit.
   5. If the remote branch exists **and** has an identical tree **and** its parent is the current base: no push needed.
      Otherwise push with `--force-with-lease`.
   6. Call `release-notes`. Failure here is a warning, not an error.
   7. Render the title and body, then `create-pull-request` or `update-pull-request`. Only call update if the title or
      body actually changed.
7. **Close:** `close-pull-request` with an explanatory comment, then delete the remote branch.
8. **Summary:** counts of created, updated, unchanged, closed and failed. Exit with status 1 if anything failed.

**Error policy:** a failure for one dependency or ecosystem is collected and the run continues. Only config,
plugin-loading and preflight failures abort the run.

### Defaults (hardcoded for now)

| Item           | Default                                                                                          |
| -------------- | ------------------------------------------------------------------------------------------------ |
| Branch         | `doup/<ecosystem>/<sanitised-dep-name>`                                                          |
| Ecosystem name | wkg package name (`doup:npm` → `npm`); file stem for `file:` refs                                |
| Commit / title | `chore(deps): update <dep> to <to>`                                                              |
| PR body        | occurrences table (manifest, from → to), release notes (truncated), link, hidden metadata marker |

- **Metadata marker:** the PR body ends with `<!-- doup: {"ecosystem":"npm","dependency":"foo","to":"1.2.3"} -->`.
  `reconcile` reads this, not the title, to tell which update a PR was for. That way Q3 keeps working when templating
  arrives. A PR without a parseable marker is treated as not created by doup and is never touched.
- **Sanitisation:** keep `[A-Za-z0-9._/-]`, replace everything else with `-`, and collapse `//` and `..`.
- **Ecosystem name clashes:** if two ecosystems end up with the same name, that's a config error.

---

## Crate layout

Two host crates. The library does the work and the binary only adapts the command line to it.

```
projects/
  doup/                  # lib crate `doup`: all logic, no clap, no printing
    src/
      lib.rs             # public API: update(), UpdateOptions, UpdateReport, Error
      error.rs           # top-level `Error` enum (thiserror)
      config.rs          # discovery, load, normalise to PluginSpec/Config
      schema.rs          # typify (moved from cli, same relative path to the schema)
      lockfile.rs
      plugins/
        mod.rs           # PluginSpec, PluginSource, naming
        fetch.rs         # wkg CachingClient + file sources, digest verification
        runtime.rs       # Engine, HostState, linker setup, limits, log import
        ecosystem.rs     # bindgen + EcosystemPlugin wrapper
        platform.rs      # bindgen + PlatformPlugin wrapper
      git.rs
      pipeline/
        mod.rs           # run(): orchestrates phases
        reconcile.rs     # pure decision logic
        render.rs        # branch names, commit msg, PR title/body
    tests/               # integration tests (fixture plugins, temp git repos, wiremock)
  cli/                   # bin crate, binary `doup`: clap + tokio main + output
    src/
      main.rs            # #[tokio::main], tracing-subscriber init, exit codes
      commands/
        update.rs        # UpdateArgs -> doup::UpdateOptions; render UpdateReport
        init.rs          # (out of scope, stays todo!())
        self_update.rs   # (out of scope, stays todo!())
      output.rs          # dry-run table + summary rendering
```

Public API of `doup` (start small; grow it with `init` later):

```rust
pub struct UpdateOptions {
  pub cwd: PathBuf,
  pub config_path: Option<PathBuf>,
  pub dry_run: bool,
  pub locked: bool,
}

/// Structured outcome. The CLI decides how to print it and which exit code to use.
pub struct UpdateReport {/* planned updates, actions taken, failures: Vec<Failure> */}

pub async fn update(opts: UpdateOptions) -> Result<UpdateReport, doup::Error>;
```

### Errors (decided: `thiserror` from the start)

The `doup` library uses typed errors built with `thiserror` and does **not** depend on `anyhow`. Only `cli` uses
`anyhow`, to add context and print the error chain. `main.rs` already prints with `{err:#}`, which walks `source()`.

- **Use one error enum per module.** Each module owns its failure modes:
  - `ConfigError`: not found, parse, invalid plugin ref.
  - `LockfileError`: read/write/parse, would change under `--locked`.
  - `FetchError`: registry, digest mismatch, re-pointed tag.
  - `InvokeError`: instantiate, trap, timeout, or a `PluginError` the plugin returned.
  - `GitError`: the command, its exit status and its stderr.
- **Name the host's invocation error `InvokeError`, not `PluginError`.** bindgen already generates a `PluginError` type
  from the WIT, and reusing the name inside `plugins/` would be confusing.
- **A top-level `doup::Error`** in `error.rs` wraps the module errors with `#[from]`. `?` then converts automatically,
  and `update()` returns `Result<_, doup::Error>`.
- **Keep the cause chain.** Use `#[source]` / `#[from]` for underlying errors such as `std::io::Error`,
  `wasm_pkg_client::Error`, wasmtime errors and `serde_saphyr` errors. Never flatten them into a `String`.
- **Mark public enums `#[non_exhaustive]`**, so adding a variant later isn't a breaking change.
- **Message style** (Rust API guidelines): lowercase, no trailing period, and say what failed rather than repeating the
  source's message. For example: `#[error("failed to load plugin {name}")]`, with the cause in `#[source]`.
- **No catch-all variants**, such as `Other(#[from] anyhow::Error)` or `Other(String)`. They undo the point of typed
  errors.
- **Per-item failures** in `UpdateReport` keep the typed error: `Failure { ecosystem: String, dependency:
  Option<String>, stage: Stage, error: doup::Error }`. The CLI can then match on it, for example to show a timeout
  differently from a plugin-reported error.

A sketch of the shape:

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
  #[error(transparent)]
  Config(#[from] ConfigError),
  #[error(transparent)]
  Lockfile(#[from] LockfileError),
  #[error(transparent)]
  Fetch(#[from] FetchError),
  #[error(transparent)]
  Invoke(#[from] InvokeError),
  #[error(transparent)]
  Git(#[from] GitError),
}

#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ConfigError {
  #[error("no doup config found between {start} and the git root; run `doup init` to create one")]
  NotFound { start: PathBuf },
  #[error("failed to parse {path}")]
  Parse {
    path: PathBuf,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync>,
  },
  #[error("invalid plugin reference `{reference}`: {reason}")]
  InvalidRef { reference: String, reason: String },
}
```

Rules of thumb:

- **The library never prints and never exits.** It emits `tracing` events and returns data. Only `cli` installs a
  `tracing-subscriber`, formats output, and calls `std::process::exit`. This keeps the library usable from tests and,
  later, from other front-ends.
- **Errors:** see [Errors](#errors-decided-thiserror-from-the-start). Typed `thiserror` errors in `doup`; `anyhow` only
  in `cli`.
- **Per-item failures belong in `UpdateReport`, not in `Err`.** `Err` is reserved for fatal failures: config, plugin
  loading and preflight.
- **Name clash:** the `cli` package's binary is called `doup`, and so is the library crate. Building is fine, but `cargo
  doc` warns about an output filename collision. Set `doc = false` on the `[[bin]]` target to avoid it.
- **Shared versions:** move common dependency versions (`tokio`, `anyhow`, `serde`, `wasmtime*`, `wit-bindgen`) into
  `[workspace.dependencies]` in the root `Cargo.toml`, and use `dep.workspace = true` in member crates.

**Crates to add:**

- **`doup` (lib):** `tokio`, `wasmtime`, `wasmtime-wasi`, `wasmtime-wasi-http`, `wasm-pkg-client`, `typify`, `regress`,
  `serde`, `serde_json`, `serde-saphyr` (replacing `serde_yaml`, note 12), `sha2`, `hex`, `toml`, `tempfile`, `tracing`,
  `thiserror`. No `anyhow`.
- **`doup` dev-dependencies:** `wiremock`.
- **`cli` (bin):** `doup` (path), `clap`, `tokio` (`macros`, `rt-multi-thread`), `tracing-subscriber`, `anyhow`. Its
  current `wasmtime*`, `typify`, `regress`, `serde*` dependencies move to `doup`.
- **Fixture plugins:** `wit-bindgen`, `serde`, `serde_json`, `wstd`.

---

## Testing strategy

- **Unit tests:**
  - config normalisation
  - plugin ref parsing
  - lockfile resolution rules
  - branch sanitisation
  - PR rendering
  - **`reconcile`** (most of the edge cases live here)
- **Fixture plugins** (in `projects/plugins/`, replacing the calculator plugins):
  - `fixture-ecosystem` reads `/workspace/**/fixture-deps.json` (`{"name": "version"}`), gets latest versions and
    release notes from a mock registry URL in its config, and rewrites the JSON in `apply-update`. This exercises
    preopens and wasi:http.
  - `fixture-platform` is a tiny REST client against a `baseUrl` from its config.
- **Integration tests** (`projects/doup/tests/`):
  - Create a temp bare "origin" repo and a clone.
  - Start a `wiremock` server.
  - Run the pipeline in-process.
  - Assert on the HTTP calls made and on the remote branches.
  - Cover: first run creates PRs, a re-run is a no-op, a newer version updates in place, a base branch move triggers a
    re-push, an up-to-date dependency closes its PR, an erroring ecosystem doesn't close anything, a PR the user closed
    stays closed until a newer version appears, running from a feature branch with local edits still discovers from
    `origin/<base>`.
- **Taskfile:**
  - `build:fixtures` runs `cargo build -p fixture-* --target wasm32-wasip2`.
  - `test` depends on it.

---

## Milestones

Each milestone should be independently mergeable and leave `cargo test` green.

### M1: Contracts & cleanup

- [ ] Delete `projects/plugins/{add,subtract,multiply,divide}` and `plugin_host.rs`; update the root `Cargo.toml`
      members.
- [ ] Create the `projects/doup` library crate with `error.rs` (top-level `Error`, `#[non_exhaustive]`). Move
      `schema.rs` and the wasmtime/typify/serde dependencies into it, make `cli` depend on it, and add
      `[workspace.dependencies]`.
- [ ] Write the `doup:plugin@0.1.0` WIT files.
- [ ] Scaffold `fixture-ecosystem` and `fixture-platform` so they compile against the WIT (stub bodies are fine).
- [ ] Update the Taskfile `build` / `run` tasks and the README status section.

### M2: Config

- [ ] Schema changes: entry shape, ref patterns, `baseBranch`.
- [ ] Swap `serde_yaml` for `serde-saphyr` (note 12). Add config discovery, the `--config` flag, YAML/JSON loading, and
      normalisation into domain types.
- [ ] `cli`: switch to `#[tokio::main]`, install `tracing-subscriber`, add `-v` and `--config` flags, and map
      `UpdateArgs` to `doup::UpdateOptions`.

### M3: Host runtime (`file:` plugins only)

- [ ] Async engine, `HostState`, WASI p2 + wasi:http linker, `log` import.
- [ ] Preopens, env allow-list, stdout→stderr, memory limits, epoch + timeout.
- [ ] `EcosystemPlugin` / `PlatformPlugin` wrappers; error mapping from `PluginError` and traps.
- [ ] Integration test: load the fixture plugins from `file:` refs and call each export.

### M4: Discovery & dry run

- [ ] Discovery and next-version phases with per-dependency error collection.
- [ ] Dry-run table output.
- [ ] Fill in `fixture-ecosystem`'s real behaviour.

### M5: Registry plugins & lockfile

- [ ] `wasm-pkg-client` resolution through `CachingClient` + `FileCache::global_cache()`, with sha256 verification and
      re-fetch on a corrupt cache entry.
- [ ] `doup.lock` read/write, plus the `--locked` flag.

### M6: Git & updates

- [ ] `git.rs` wrapper, `WorktreeGuard`, preflight (fetch, pin base SHA, discovery worktree).
- [ ] Apply → commit → tree compare → force-with-lease push.

### M7: Platform reconciliation

- [ ] `reconcile()` with exhaustive unit tests.
- [ ] Create/update/close PRs, delete stale branches, release notes in the PR body.
- [ ] Fill in `fixture-platform`; full end-to-end integration scenarios.

### M8: Polish

- [ ] Summary output and exit codes; review error messages.
- [ ] Document the plugin author guide (the WIT contract, the `/workspace` convention, config as JSON).

---

## Future work (noted, not planned)

- Per-plugin network allow-lists, enforced through `WasiHttpHooks`.
- Injecting auth headers in the host so plugins never see tokens.
- Templated branch names, commit messages and PR bodies.
- Grouping, filtering, schedules, rate limits.
- Parallel plugin calls.
- Caching compiled components (`Component::serialize`).
- A `config-schema` export so config errors are caught up front. It could also feed a generated, plugin-aware JSON
  Schema for editor autocomplete, similar to dprint. Deferred as too much effort for now.
- A `registries` section in `doup.yaml`, so CI doesn't need a wkg `config.toml` for third-party namespaces.
- Hosting `/.well-known/wasm-pkg/registry.json` on a doup domain, so the registry can move without a doup release.
- Publishing the WIT package for third-party plugin authors.
- JSONC and TOML config.

---

## Decisions

- **Q1: plugin entry shape.** `{ plugin, env, config }` with a string short form. `config` is any YAML/JSON value,
  passed to the plugin as a JSON string. No schema export for now.
- **Q2: registry.** A built-in `doup` → `ghcr.io/brad-jones/` mapping that the user's wkg config can override.
- **Q3: user-closed PRs.** Skip until a newer version is available. The target version is detected through the PR body's
  metadata marker.
- **Q4: preflight.** Always discover from a detached worktree at the pinned `origin/<base>` SHA. Dry runs read the
  current checkout instead.
- **Q5: lockfile format.** TOML.
- **YAML parser.** `serde-saphyr` with default features, and `strict_booleans: true` when reading (note 12).
- **Plugin cache.** wkg's `CachingClient` + `FileCache::global_cache()` (shared with the `wkg` CLI). doup adds digest
  verification against `doup.lock`, because `FileCache` doesn't verify on write.
- **Crate split.** A `doup` library crate holds all the logic. `cli` is a thin clap/tokio wrapper that owns output and
  exit codes.
- **Errors.** `doup` uses typed `thiserror` enums (one per module, wrapped by a `#[non_exhaustive]` top-level `Error`).
  `anyhow` is used only in `cli`.

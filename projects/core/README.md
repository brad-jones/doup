# doup-core

The heart of doup: updates the dependencies of every repo the bot is configured for, opening, updating & closing pull
requests as needed. The CLI's `doup update` is a thin wrapper around this crate.

## Usage

```rust
use doup_config::{bot::DoupBotConfigFile, file::ConfigFile};
use doup_core::{ActionOutcome, Updater};

let config = DoupBotConfigFile::load(None, None)?.merged;
let report = Updater::from_config(&config) // git backend, repos & concurrency from the bot config
  .dry_run(false) // true: work everything out, but push nothing & touch no pull request
  .run()
  .await;

for repo in report.repos() {
  for action in repo.actions() {
    println!("{} {} ({:?})", repo.origin(), action.action(), action.outcome());
    println!("  because: {}", action.action().reason());
  }
}
if !report.is_success() {
  eprintln!("{} repo(s) failed", report.failed_repos().count());
}
```

`Updater::new(git, repos)` builds one without a config file, and `.concurrency(..)` & `.plugins(..)` override the
defaults. `run` never fails as a whole, see [Errors](#errors).

## How a repo is updated

Every repo goes through the same pipeline, in its own tokio task:

1. **Clone** the repo (depth 1) into a temp dir, with the auth from the bot config.
2. **Load its config** (`doup.yaml`, plus `DOUP_REPO_*` & `DOUP_REPO_<index>_*` env overrides, see `doup-config`).
3. **Load its plugins**, the platform & every ecosystem, concurrently. A bad config or module fails here, before any
   work starts.
4. **Discover** the dependencies, with every ecosystem concurrently.
5. **Look up the versions** of every dependency, concurrently.
6. **Observe** the repo's existing work, concurrently: inspect every `doup/` branch (who committed to it & whether it is
   behind the default branch), list the open doup pull requests, & list the closed pull requests of the available
   updates.
7. **Reconcile**: decide what to do, see [below](#reconciliation). This step is pure.
8. **Execute**, unless it's a dry run: build & push each update then open or update its pull request, then close pull
   requests, then delete branches.

## Concurrency

| Limit                       | Default | Bounds                                                                                  |
| --------------------------- | ------- | --------------------------------------------------------------------------------------- |
| `concurrency.repos`         | 4       | Repos updated at once.                                                                  |
| `concurrency.plugin_calls`  | 8       | Plugin calls in flight per repo: version lookups, release notes & pull request changes. |
| `concurrency.apply_updates` | 2       | `apply_updates` calls across **every** repo, as they may run containers (eg: `cargo`).  |

They're set in the bot config (or `DOUP_BOT_CONCURRENCY__*` env vars), see `doup-config`.

Git isn't built for concurrent writes to one clone: shallow fetches & worktree bookkeeping take locks that fail, rather
than wait, when contended. So git operations on the same clone run one at a time behind an async mutex, while plugin
calls (the slow part: HTTP & containers) run concurrently outside of it. Each update is built in a worktree of its own,
so several updates of one repo can be applied at once. Different repos are different clones, so never wait on each
other.

## Reconciliation

`reconcile::reconcile` takes a `RepoState` & returns a list of `Action`s, each with a reason. Each dependency with an
update available gets up to two updates, each with a branch (`doup/<plugin>/<next|latest>/<dependency>-<hash>`, see
`doup_git::UpdateBranch`) & pull request of its own:

- `next`: the newest version within its version constraints, unless that's the version already used.
- `latest`: the newest version of all, likely with breaking changes, unless that's the version already used or `next`.

Then, for each update:

| Existing pull request on its branch              | Action                                                 |
| ------------------------------------------------ | ------------------------------------------------------ |
| None                                             | `CreatePr`                                             |
| Open, someone other than doup has pushed to it   | `Skip`, never overwrite someone's work                 |
| Open, at another version                         | `UpdatePr`                                             |
| Open, at this version, behind the default branch | `UpdatePr`, to re-run its checks on the latest changes |
| Open, at this version, up to date                | `Skip`                                                 |
| Closed (any of them) at this version             | `Skip`, that's how a maintainer ignores a version      |
| Closed, at other versions only                   | `CreatePr`                                             |

Then cleanup:

- Every open `doup/` pull request no update claims is closed (`ClosePr`).
- Every branch passed in `RepoState::branches` (doup only branches) that no remaining pull request or update uses is
  deleted (`DeleteBranch`): branches of pull requests being closed, of closed pull requests & orphans (eg: merged).
- Nothing on a branch in `RepoState::protected_branches` is ever closed or deleted. The branches of a dependency whose
  versions couldn't be looked up are protected, as there's no telling if its pull requests are still wanted.

### Reasons

Each action's reason is a typed enum (`CreateReason`, `UpdateReason`, `SkipReason`, `CloseReason` & `DeleteReason`) that
renders as sentences written for the repo's maintainers, eg: _"Pull request #7, which proposed this version, was closed
without being merged, so doup won't propose it again."_ They're logged & reported, a `CloseReason` is posted as a
comment on the pull request being closed and an `UpdateReason` is noted in the updated pull request's body.

## Pull requests

The title (& commit message) is `chore(doup): update <name> from <current> to <version>`. The body, see `content`, has:

- A summary of the update, with a warning on a `latest` update that it may contain breaking changes.
- A note pointing at the dependency's other update, when it has one.
- A table of the dependency, ecosystem, registry & versions, and one of every manifest & lockfile it's used in.
- The release notes, newest first, in collapsible sections (the newest open). Release notes are fetched once per
  dependency, up to its newest update, and each pull request only shows those up to its own version (compared as semver,
  leniently, notes that can't be compared are kept). They're cut down to keep the body within `content::MAX_BODY_LEN`:
  older notes become links, then a count. Failing to fetch them never fails the pull request.
- A footer explaining how doup manages the pull request.

## Errors

Nothing stops the whole run, every error is confined to what it hit & recorded in the `UpdateReport`:

- `RepoError`: the repo couldn't be updated at all (eg: clone, config or plugin loading failed). Other repos carry on.
- `DependencyError`: a dependency's versions couldn't be looked up. Its pull requests & branches are left alone, the
  rest of the repo carries on.
- `ActionError`: an action failed (`ActionOutcome::Failed`). The other actions carry on, except a branch is not deleted
  when closing its pull request failed (`ActionOutcome::NotAttempted`), as that would close the pull request without the
  comment explaining why.

`RepoReport::is_success` & `UpdateReport::is_success` are `false` when any of them happened. All errors are `thiserror`
enums whose messages only describe their own context, walk `std::error::Error::source` (or render with anyhow's `{:#}`)
for the whole chain.

## Logging

Progress is reported with [`tracing`](https://docs.rs/tracing), the de facto standard for structured logging in async
Rust. Its spans give every event the context it happened in, which keeps the interleaved output of concurrent work
readable. Install a subscriber, eg: [`tracing-subscriber`](https://docs.rs/tracing-subscriber) as the CLI does, to see
it.

| Span         | Fields              | Around                                   |
| ------------ | ------------------- | ---------------------------------------- |
| `repo`       | `origin` (redacted) | Everything done to a repo.               |
| `ecosystem`  | `name`              | Discovering an ecosystem's dependencies. |
| `dependency` | `name`, `ecosystem` | Looking up a dependency's versions.      |
| `action`     | `action`            | Carrying out an action.                  |

Events are `info` for progress (eg: `update available`, `planned` with `action` & `reason`, `done` with `url`), `debug`
for detail, `warn` for problems that don't fail anything (eg: release notes couldn't be fetched) & `error` for failures,
with the error in an `error` field.

## Plugins & testing

The plugins are used through the `plugins::Platform`, `plugins::Ecosystem` & `plugins::PluginLoader` traits.
`plugins::WasmPlugins`, the default, loads the real WASM plugins. The integration tests (`tests/`) swap in fakes that
record what doup did, & run the whole pipeline against real git remotes on disk with both git backends.

```sh
task test:unit        # in-crate tests: reconciliation, pull request content & release notes
task test:integration # the whole pipeline, with fake plugins
```

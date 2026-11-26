# git

The Git operations doup needs, behind one interface. The rest of the workspace depends on this crate and never on Git
itself.

## Usage

Pick a backend when building a `Git`, then clone. That gives back a `ClonedRepo` with operations on the new clone:

```rust
use doup_git::{Auth, CloneOptions, Git, GitCli, Grit};
use std::num::NonZeroU32;

let git = Git::new(GitCli); // or Git::new(Grit)

let options = CloneOptions::new("https://github.com/owner/repo.git", "repo")
  .with_branch("main")
  .with_depth(NonZeroU32::new(1).unwrap())
  .with_auth(Auth::Https { user: "x-access-token".into(), pass: token });

let repo = git.clone_repo(&options).await?;
let branches = repo.list_remote_branches().await?; // like `git ls-remote --heads origin`
let default = repo.default_branch().await?; // the branch origin's HEAD points at, e.g. "main"
let commits = repo.branch_commits("doup/foo").await?; // like `git log origin/HEAD..origin/doup/foo`
let authors = repo.branch_authors("doup/foo").await?; // their `%ae`, sorted and deduplicated
```

Every operation is async (built on `tokio`) and fails with a typed `GitError`.

`ClonedRepo` keeps the clone's `Auth` in memory (never on disk, and redacted from `Debug`), so later operations reach
`origin` with the same credentials.

### A branch's own commits

`branch_fetch`, `branch_commits` and `branch_authors` deal with the commits a branch has on top of `origin`'s default
branch, and work whatever state the clone is in (shallow, single-branch, branch created or force-pushed since cloning):

- `branch_fetch` fetches only those commits, without file contents: `git fetch --filter=blob:none
  --shallow-exclude=refs/heads/<default> origin +refs/heads/<branch>:refs/remotes/origin/<branch>`. This makes the clone
  a partial clone (`remote.origin.promisor`).
- `branch_commits` lists them newest first as `GitCommit`s (`id`, author name and email, `message`, `is_merge`), read
  from the raw commit objects. `git log` would hide the parents of a commit on a shallow boundary, getting `is_merge`
  wrong.
- A branch whose tip is the default branch's tip (including the default branch itself) has no commits: nothing is
  fetched and `branch_commits` is empty.
- A branch strictly behind the default branch (e.g. merged) fails with `GitError::NoUniqueCommits`: the server refuses a
  shallow-exclude fetch that would send nothing, and over smart HTTP (e.g. GitHub) the refusal is too vague to tell
  apart from other failures, so it is not turned into an empty list.
- **Merges stop the walk.** Shallow boundaries are per commit, so a merge of the default branch into the branch is a
  boundary and the commits behind it are not fetched or listed. The merge itself is listed with `is_merge()` set, so a
  caller can treat any merge as "the list may be incomplete".

## Backends

| Backend  | How it works                                                                      | Needs on `PATH`        |
| -------- | --------------------------------------------------------------------------------- | ---------------------- |
| `GitCli` | Runs the real `git` executable via `tokio::process`.                              | `git` (+ `ssh` if SSH) |
| `Grit`   | Runs [grit](https://grit-scm.com) in-process via `grit-lib`, on `spawn_blocking`. | `ssh` if SSH           |

`Git` is an enum with one variant per backend, dispatched with [spire_enum](https://crates.io/crates/spire_enum). Its
methods, and `ClonedRepo`'s, validate input before delegating to the backend's `GitBackend` impl and normalise what
comes back, so both backends behave the same way. Both must pass the same contract tests (see `backend_contract_tests!`
in `src/test_support.rs`). A new backend is a new `Git` variant.

### Working with grit

grit is a Git implementation in Rust, used here through the `grit-lib` crate. **Before changing the `Grit` backend, read
<https://grit-scm.com/llms.txt>**: it indexes the library guide, the full API docs and the CLI reference.

Notes from building this crate against `grit-lib` 0.5.x:

- `grit_lib::remote::clone::clone` is synchronous, so it runs inside `tokio::task::spawn_blocking` with a `CancelToken`
  that is cancelled if the awaiting future is dropped. Listing remote refs has no `CancelToken`, so it just runs to
  completion.
- grit has no in-process SSH. It always spawns `ssh` (via `sh -c` for `GIT_SSH_COMMAND`), resolving the command from the
  `Environment` on its `CloneOptions`, or for an existing repo the `Environment` it was opened with. That `Environment`
  is **not** passed on to the `ssh` child, so anything ssh needs must be inlined in the command itself.
- HTTPS credentials go through a custom `HttpClientFactory` + `CredentialProvider`, so they stay in memory and are not
  written to config.
- A failed clone is not cleaned up by grit; call `should_cleanup_failed_clone` / `cleanup_failed_clone`.
- Map grit errors (`Error::Remote(RemoteError::..)`) onto `GitError` variants rather than exposing them directly.
- `FetchOptions::environment` defaults to empty, so set it to the repo's `Environment` like the rest. Only `blob:none`
  and `blob:limit` filters exist (no `tree:0`). `deepen_not` is the `--shallow-exclude` option.
- grit's in-process upload-pack, which serves local and `file://` remotes, does not refuse a `deepen_not` fetch that
  selects nothing as git does: it sends the whole history instead. So `fetch_branch` checks for that case itself before
  fetching from a local remote.
- Walks (`revwalk().commits`) honour shallow grafts, so read raw objects (`objects().read`) when the real parents
  matter.

## Authentication

`Auth` mirrors the bot config's `DoupBotRepoAuth`:

- `Auth::Https { user, pass }`: `GitCli` sends a basic auth `extraheader` through `GIT_CONFIG_*` environment variables
  (never in args), and `Grit` answers auth challenges from memory, only for the remote's own origin.
- `Auth::Ssh { key }`: the key is held by an ssh-agent served from doup's own process (`ssh-agent-lib`), so it never
  touches disk. Only the public key and the agent's socket live in an owner-only temp dir, so a SIGKILL leaves nothing
  secret behind. Both backends use `SSH_AUTH_SOCK=<socket> ssh -o IdentityAgent=SSH_AUTH_SOCK -o IdentitiesOnly=yes -i
  <key>.pub -o BatchMode=yes -o StrictHostKeyChecking=accept-new`, so only this key is ever offered.
  - On Windows the agent is a randomly named pipe and the command runs Windows' own OpenSSH
    (`%SystemRoot%\System32\OpenSSH\ssh.exe`), because the MSYS ssh bundled with Git for Windows cannot reach named
    pipes. The pipe is named in a small `ssh_config` beside the public key and passed with `-F`, as the MSYS shell that
    runs `GIT_SSH_COMMAND` mangles a `\\.\pipe\...` path given via the environment or arguments.
  - Ed25519, ECDSA and RSA (`rsa-sha2-256`/`rsa-sha2-512`, not SHA-1) keys are supported.

Without `Auth`, the user's existing Git and SSH setup is used.

## Adding an operation

Only add operations doup actually needs:

1. Add a method to the `GitBackend` trait. Operations on a clone take the `&ClonedRepo`.
2. Add the public method that callers use: on `Git` for operations without a repo, or on `ClonedRepo` for operations on
   a clone. It validates input, forwards via `delegate_git!(...)`, and normalises the result if the backends could
   differ, e.g. in ordering.
3. Implement it for both `GitCli` and `Grit`, mapping failures to `GitError`.
4. Add contract tests to `backend_contract_tests!` so both backends are held to the same behaviour.

## Testing

```sh
task test:unit        # in-crate tests; needs the system `git` and `ssh-keygen`
task test:integration # tests/ dir; clones from GitHub over the network
```

The SSH integration test needs a repo and a read-only deploy key for it. It is skipped locally unless both are set (eg:
in `.env`), and fails in CI (`CI` set) if they are missing:

```sh
DOUP_TEST_SSH_URL=git@github.com:owner/repo.git
DOUP_TEST_SSH_KEY="-----BEGIN OPENSSH PRIVATE KEY-----..."
```

CI runs the integration tests natively on every build target, which is what validates the Windows named pipe.

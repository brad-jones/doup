# git

The Git operations doup needs, behind one interface. The rest of the workspace depends on this crate and never on Git
itself.

## Usage

Pick a backend when building a `Git`, then call operations on it:

```rust
use git::{Auth, CloneOptions, Git, GitCli, Grit};
use std::num::NonZeroU32;

let git = Git::new(GitCli); // or Git::new(Grit)

let options = CloneOptions::new("https://github.com/owner/repo.git", "repo")
  .with_branch("main")
  .with_depth(NonZeroU32::new(1).unwrap())
  .with_auth(Auth::Https { user: "x-access-token".into(), pass: token });

git.clone_repo(options).await?;
```

Every operation is async (built on `tokio`) and fails with a typed `GitError`.

## Backends

| Backend  | How it works                                                                      | Needs on `PATH`        |
| -------- | --------------------------------------------------------------------------------- | ---------------------- |
| `GitCli` | Runs the real `git` executable via `tokio::process`.                              | `git` (+ `ssh` if SSH) |
| `Grit`   | Runs [grit](https://grit-scm.com) in-process via `grit-lib`, on `spawn_blocking`. | `ssh` if SSH           |

`Git` validates options before delegating, so both backends reject bad input the same way. Both must pass the same
contract tests (see `backend_contract_tests!` in `src/test_support.rs`).

### Working with grit

grit is a Git implementation in Rust, used here through the `grit-lib` crate. **Before changing the `Grit` backend, read
<https://grit-scm.com/llms.txt>**: it indexes the library guide, the full API docs and the CLI reference.

Notes from building this crate against `grit-lib` 0.5.x:

- `grit_lib::remote::clone::clone` is synchronous, so it runs inside `tokio::task::spawn_blocking` with a `CancelToken`
  that is cancelled if the awaiting future is dropped.
- grit has no in-process SSH. It always spawns `ssh` (via `sh -c` for `GIT_SSH_COMMAND`), resolving the command from the
  `Environment` on its `CloneOptions`. That `Environment` is **not** passed on to the `ssh` child, so anything ssh needs
  must be inlined in the command itself.
- HTTPS credentials go through a custom `HttpClientFactory` + `CredentialProvider`, so they stay in memory and are not
  written to config.
- A failed clone is not cleaned up by grit; call `should_cleanup_failed_clone` / `cleanup_failed_clone`.
- Map grit errors (`Error::Remote(RemoteError::..)`) onto `GitError` variants rather than exposing them directly.

## Authentication

`Auth` mirrors the bot config's `DoupBotRepoAuth`:

- `Auth::Https { user, pass }`: `GitCli` sends a basic auth `extraheader` through `GIT_CONFIG_*` environment variables
  (never in args), and `Grit` answers auth challenges from memory, only for the clone's own origin.
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

1. Add a method to the `GitBackend` trait and a validating wrapper on `Git`.
2. Implement it for both `GitCli` and `Grit`, mapping failures to `GitError`.
3. Add contract tests to `backend_contract_tests!` so both backends are held to the same behaviour.

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

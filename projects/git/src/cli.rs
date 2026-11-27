use crate::auth::basic_auth_header;
use crate::remote_url::{RemoteUrl, redact};
use crate::ssh_agent::SshAgent;
use crate::{Auth, CloneOptions, ClonedRepo, GitBackend, GitCommit, GitError, RemoteTips, Worktree};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Shells out to the `git` executable found on `PATH`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitCli;

impl GitBackend for GitCli {
  async fn clone_repo(&self, options: &CloneOptions) -> Result<(), GitError> {
    // Must outlive the child process, which signs through the agent.
    let agent = SshAgent::for_auth(options.auth())?;
    let invocation = clone_invocation(options, agent.as_ref())?;
    let result = run(invocation, |stderr| classify_clone_failure(options, stderr)).await;
    drop(agent);
    result.map(drop)
  }

  async fn list_remote_branches(&self, repo: &ClonedRepo) -> Result<Vec<String>, GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let invocation = ls_remote_invocation(repo, agent.as_ref())?;
    let result = run(invocation, |stderr| classify_failure(repo.url(), stderr)).await;
    drop(agent);
    Ok(parse_ls_remote(&String::from_utf8_lossy(&result?)))
  }

  async fn remote_tips(&self, repo: &ClonedRepo, branch: Option<&str>) -> Result<RemoteTips, GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let invocation = ls_remote_tips_invocation(repo, branch, agent.as_ref())?;
    let result = run(invocation, |stderr| classify_failure(repo.url(), stderr)).await;
    drop(agent);
    parse_remote_tips(&String::from_utf8_lossy(&result?), branch)
      .ok_or_else(|| GitError::NoDefaultBranch { url: redact(repo.url()) })
  }

  async fn fetch_branch(&self, repo: &ClonedRepo, branch: &str, exclude: &str) -> Result<(), GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let invocation = fetch_branch_invocation(repo, branch, exclude, agent.as_ref())?;
    let result = run(invocation, |stderr| classify_fetch_branch_failure(repo.url(), branch, stderr)).await;
    drop(agent);
    result.map(drop)
  }

  async fn branch_commits(&self, repo: &ClonedRepo, branch: &str) -> Result<Vec<GitCommit>, GitError> {
    let local = |args: &[&str], stdin: Option<Vec<u8>>| Invocation {
      args: args.iter().map(Into::into).collect(),
      envs: Vec::new(),
      current_dir: Some(repo.path().to_path_buf()),
      stdin,
    };
    let tip = format!("refs/remotes/origin/{branch}");
    let ids = run(local(&["rev-list", &tip, "--"], None), |_| None).await?;
    if ids.is_empty() {
      return Ok(Vec::new());
    }
    // Raw objects, as `git log` hides the parents of commits on a shallow boundary.
    let objects = run(local(&["cat-file", "--batch"], Some(ids)), |_| None).await?;
    parse_cat_file_batch(&objects)
  }

  async fn tracking_tip(&self, repo: &ClonedRepo, branch: &str) -> Result<Option<String>, GitError> {
    let tracking = format!("refs/remotes/origin/{branch}^{{commit}}");
    let invocation = in_clone(repo, ["rev-parse", "--quiet", "--verify", "--end-of-options", &tracking]);
    match run(invocation, |_| None).await {
      Ok(stdout) => Ok(Some(String::from_utf8_lossy(&stdout).trim().to_owned())),
      // `--verify --quiet` exits 1, saying nothing, when the ref does not exist.
      Err(GitError::CommandFailed { status, .. }) if status.code() == Some(1) => Ok(None),
      Err(err) => Err(err),
    }
  }

  async fn delete_remote_branch(&self, repo: &ClonedRepo, branch: &str, lease: &str) -> Result<(), GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let invocation = delete_invocation(repo, branch, lease, agent.as_ref())?;
    let result = run(invocation, |stderr| classify_push_failure(repo.url(), branch, stderr)).await;
    drop(agent);
    result?;
    // Push only prunes the tracking ref when the fetch refspec covers it, which a single-branch
    // clone's does not. Deleting a missing ref succeeds.
    let tracking = format!("refs/remotes/origin/{branch}");
    run(in_clone(repo, ["update-ref", "-d", &tracking]), |_| None).await.map(drop)
  }

  async fn worktree_add(&self, worktree: &Worktree) -> Result<(), GitError> {
    run(worktree_add_invocation(worktree), |_| None).await.map(drop)
  }

  async fn worktree_commit(&self, worktree: &Worktree, message: &str) -> Result<String, GitError> {
    run(in_worktree(worktree, ["add", "--all"]), |_| None).await?;
    // Exits 1 when something is staged.
    match run(in_worktree(worktree, ["diff", "--cached", "--quiet"]), |_| None).await {
      Ok(_) => return Err(GitError::NothingToCommit),
      Err(GitError::CommandFailed { status, .. }) if status.code() == Some(1) => {}
      Err(err) => return Err(err),
    }
    run(commit_invocation(worktree, message), |_| None).await?;
    branch_tip(worktree).await
  }

  async fn worktree_push(&self, worktree: &Worktree) -> Result<String, GitError> {
    push(worktree, Push::FastForward).await
  }

  async fn worktree_push_force(&self, worktree: &Worktree, lease: Option<&str>) -> Result<String, GitError> {
    push(worktree, Push::ForceWithLease(lease)).await
  }

  async fn worktree_remove(&self, worktree: &Worktree) -> Result<(), GitError> {
    let mut invocation = in_worktree(worktree, ["worktree", "remove", "--force"]);
    invocation.args.push(worktree.path().into());
    invocation.current_dir = Some(worktree.repo().path().to_path_buf());
    run(invocation, |_| None).await.map(drop)
  }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Push<'a> {
  FastForward,
  /// Overwrite the branch as long as `origin` still has it at this id, or not at all for `None`.
  ForceWithLease(Option<&'a str>),
}

async fn push(worktree: &Worktree, mode: Push<'_>) -> Result<String, GitError> {
  // Pushing this exact id means what is returned is what was pushed.
  let id = branch_tip(worktree).await?;
  let repo = worktree.repo();
  let agent = SshAgent::for_auth(repo.auth())?;
  let invocation = push_invocation(worktree, &id, mode, agent.as_ref())?;
  let result = run(invocation, |stderr| classify_push_failure(repo.url(), worktree.branch(), stderr)).await;
  drop(agent);
  result.map(|_| id)
}

async fn branch_tip(worktree: &Worktree) -> Result<String, GitError> {
  let tip = format!("refs/heads/{}", worktree.branch());
  let stdout = run(in_worktree(worktree, ["rev-parse", "--verify", "--end-of-options", &tip]), |_| None).await?;
  Ok(String::from_utf8_lossy(&stdout).trim().to_owned())
}

#[derive(Debug)]
struct Invocation {
  args: Vec<OsString>,
  envs: Vec<(&'static str, OsString)>,
  current_dir: Option<PathBuf>,
  stdin: Option<Vec<u8>>,
}

/// Runs git and returns its stdout, turning a failure into the error `classify` reads from stderr.
async fn run(invocation: Invocation, classify: impl FnOnce(&str) -> Option<GitError>) -> Result<Vec<u8>, GitError> {
  let mut command = Command::new("git");
  if let Some(dir) = &invocation.current_dir {
    command.current_dir(dir);
  }
  let mut child = command
    .args(&invocation.args)
    .envs(invocation.envs)
    .stdin(if invocation.stdin.is_some() { Stdio::piped() } else { Stdio::null() })
    .stdout(Stdio::piped())
    .stderr(Stdio::piped())
    .kill_on_drop(true)
    .spawn()
    .map_err(|err| match err.kind() {
      ErrorKind::NotFound => GitError::GitNotInstalled,
      _ => GitError::Io(err),
    })?;

  // Written while stdout is read, so neither pipe can fill up and block git.
  let pipe = child.stdin.take();
  let feed = async move {
    if let (Some(mut pipe), Some(input)) = (pipe, invocation.stdin) {
      pipe.write_all(&input).await?;
    }
    Ok::<_, std::io::Error>(())
  };
  let (fed, output) = tokio::join!(feed, child.wait_with_output());
  let output = output?;

  if output.status.success() {
    fed?;
    return Ok(output.stdout);
  }
  let stderr = String::from_utf8_lossy(&output.stderr);
  Err(
    classify(&stderr)
      .unwrap_or_else(|| GitError::CommandFailed { status: output.status, stderr: stderr.trim().to_owned() }),
  )
}

fn clone_invocation(options: &CloneOptions, agent: Option<&SshAgent>) -> Result<Invocation, GitError> {
  let mut args: Vec<OsString> = vec!["clone".into(), "--quiet".into()];
  if let Some(branch) = options.branch() {
    args.push(format!("--branch={branch}").into());
  }
  if let Some(depth) = options.depth() {
    args.push(format!("--depth={depth}").into());
  }
  args.extend(["--".into(), options.url().into(), options.destination().into()]);
  Ok(Invocation { args, envs: auth_envs(options.url(), options.auth(), agent)?, current_dir: None, stdin: None })
}

fn ls_remote_invocation(repo: &ClonedRepo, agent: Option<&SshAgent>) -> Result<Invocation, GitError> {
  Ok(Invocation {
    args: vec!["ls-remote".into(), "--heads".into(), "origin".into()],
    envs: auth_envs(repo.url(), repo.auth(), agent)?,
    current_dir: Some(repo.path().to_path_buf()),
    stdin: None,
  })
}

fn ls_remote_tips_invocation(
  repo: &ClonedRepo,
  branch: Option<&str>,
  agent: Option<&SshAgent>,
) -> Result<Invocation, GitError> {
  let mut args: Vec<OsString> = vec!["ls-remote".into(), "--symref".into(), "origin".into(), "HEAD".into()];
  if let Some(branch) = branch {
    args.push(format!("refs/heads/{branch}").into());
  }
  Ok(Invocation {
    args,
    envs: auth_envs(repo.url(), repo.auth(), agent)?,
    current_dir: Some(repo.path().to_path_buf()),
    stdin: None,
  })
}

fn fetch_branch_invocation(
  repo: &ClonedRepo,
  branch: &str,
  exclude: &str,
  agent: Option<&SshAgent>,
) -> Result<Invocation, GitError> {
  Ok(Invocation {
    args: vec![
      "fetch".into(),
      "--quiet".into(),
      "--no-tags".into(),
      "--filter=blob:none".into(),
      format!("--shallow-exclude=refs/heads/{exclude}").into(),
      "origin".into(),
      format!("+refs/heads/{branch}:refs/remotes/origin/{branch}").into(),
    ],
    envs: auth_envs(repo.url(), repo.auth(), agent)?,
    current_dir: Some(repo.path().to_path_buf()),
    stdin: None,
  })
}

/// Runs git `args` in the clone itself, without touching the remote.
fn in_clone<'a>(repo: &ClonedRepo, args: impl IntoIterator<Item = &'a str>) -> Invocation {
  Invocation {
    args: args.into_iter().map(Into::into).collect(),
    envs: Vec::new(),
    current_dir: Some(repo.path().to_path_buf()),
    stdin: None,
  }
}

/// Deletes `branch` on `origin` with a lease. `--no-verify` as the user's `pre-push` hook (e.g.
/// from a global `core.hooksPath`) must never run.
fn delete_invocation(
  repo: &ClonedRepo,
  branch: &str,
  lease: &str,
  agent: Option<&SshAgent>,
) -> Result<Invocation, GitError> {
  let dst = format!("refs/heads/{branch}");
  let lease = format!("--force-with-lease={dst}:{lease}");
  let refspec = format!(":{dst}");
  let mut invocation = in_clone(repo, ["push", "--quiet", "--no-verify", &lease, "origin", &refspec]);
  invocation.envs = auth_envs(repo.url(), repo.auth(), agent)?;
  Ok(invocation)
}

/// Runs git `args` in `worktree` as the clone's identity, with an empty hooks path so none of the
/// user's hooks run.
fn in_worktree<'a>(worktree: &Worktree, args: impl IntoIterator<Item = &'a str>) -> Invocation {
  let mut hooks_path = OsString::from("core.hooksPath=");
  hooks_path.push(worktree.hooks_path());
  let mut all: Vec<OsString> = vec!["-c".into(), hooks_path];
  all.extend(args.into_iter().map(Into::into));
  let envs = worktree.repo().identity().envs().into_iter().map(|(key, value)| (key, value.into())).collect();
  Invocation { args: all, envs, current_dir: Some(worktree.path().to_path_buf()), stdin: None }
}

fn worktree_add_invocation(worktree: &Worktree) -> Invocation {
  let mut invocation = in_worktree(worktree, ["worktree", "add", "--quiet", "--no-track", "-B", worktree.branch()]);
  invocation.args.extend([worktree.path().into(), "HEAD".into()]);
  invocation.current_dir = Some(worktree.repo().path().to_path_buf());
  invocation
}

fn commit_invocation(worktree: &Worktree, message: &str) -> Invocation {
  let message = format!("--message={message}");
  in_worktree(worktree, ["-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", &message])
}

fn push_invocation(
  worktree: &Worktree,
  id: &str,
  mode: Push<'_>,
  agent: Option<&SshAgent>,
) -> Result<Invocation, GitError> {
  let dst = format!("refs/heads/{}", worktree.branch());
  let lease = match mode {
    Push::FastForward => None,
    Push::ForceWithLease(lease) => Some(format!("--force-with-lease={dst}:{}", lease.unwrap_or_default())),
  };
  let refspec = format!("{id}:{dst}");
  let args = ["push", "--quiet", "--no-verify"].into_iter().chain(lease.as_deref()).chain(["origin", &refspec]);
  let mut invocation = in_worktree(worktree, args);
  let repo = worktree.repo();
  invocation.envs.extend(auth_envs(repo.url(), repo.auth(), agent)?);
  Ok(invocation)
}

/// Secrets only ever go in the environment, never in args that `ps` can show.
fn auth_envs(
  url: &str,
  auth: Option<&Auth>,
  agent: Option<&SshAgent>,
) -> Result<Vec<(&'static str, OsString)>, GitError> {
  let mut envs = vec![("GIT_TERMINAL_PROMPT", "0".into())];
  if let Some(Auth::Https { user, pass }) = auth {
    let key = RemoteUrl::parse(url)?.extra_header_key().ok_or_else(|| GitError::InvalidUrl { url: redact(url) })?;
    envs.extend([
      ("GIT_CONFIG_COUNT", "1".into()),
      ("GIT_CONFIG_KEY_0", key.into()),
      ("GIT_CONFIG_VALUE_0", basic_auth_header(user, pass).into()),
    ]);
  }
  if let Some(agent) = agent {
    envs.extend([
      ("GIT_SSH_COMMAND", agent.ssh_command()?.into()),
      // The leading `SSH_AUTH_SOCK=` would otherwise make git probe with `ssh -G`.
      ("GIT_SSH_VARIANT", "ssh".into()),
    ]);
  }
  Ok(envs)
}

/// Reads branch names from `git ls-remote --heads` lines, `<oid>\trefs/heads/<name>`.
fn parse_ls_remote(stdout: &str) -> Vec<String> {
  stdout
    .lines()
    .filter_map(|line| line.split_once('\t'))
    .filter_map(|(_, name)| name.trim_end().strip_prefix("refs/heads/"))
    .map(str::to_owned)
    .collect()
}

/// Reads `git ls-remote --symref origin HEAD refs/heads/<branch>` output, `None` when `HEAD` does
/// not point at a branch with a commit.
fn parse_remote_tips(stdout: &str, branch: Option<&str>) -> Option<RemoteTips> {
  let branch_ref = branch.map(|branch| format!("refs/heads/{branch}"));
  let (mut default_branch, mut default_tip, mut branch_tip) = (None, None, None);
  for (value, name) in stdout.lines().filter_map(|line| line.trim_end().split_once('\t')) {
    if let Some(target) = value.strip_prefix("ref: ") {
      if name == "HEAD" {
        default_branch = target.strip_prefix("refs/heads/");
      }
    } else if name == "HEAD" {
      default_tip = Some(value);
    } else if branch_ref.as_deref() == Some(name) {
      branch_tip = Some(value);
    }
  }
  Some(RemoteTips {
    default_branch: default_branch?.to_owned(),
    default_tip: default_tip?.to_owned(),
    branch_tip: branch_tip.map(str::to_owned),
  })
}

/// Reads `git cat-file --batch` output: per object, `<id> <type> <size>\n<content>\n`.
fn parse_cat_file_batch(mut stdout: &[u8]) -> Result<Vec<GitCommit>, GitError> {
  let mut commits = Vec::new();
  while !stdout.is_empty() {
    let header_end = stdout.iter().position(|&byte| byte == b'\n').unwrap_or(stdout.len());
    let header = String::from_utf8_lossy(&stdout[..header_end]);
    let mut fields = header.split(' ');
    let (id, kind, size) = (fields.next().unwrap_or_default(), fields.next(), fields.next());
    let invalid = || GitError::InvalidCommit { id: id.to_owned() };
    let size: usize = size.and_then(|size| size.parse().ok()).filter(|_| kind == Some("commit")).ok_or_else(invalid)?;
    let content = stdout.get(header_end + 1..header_end + 1 + size).ok_or_else(invalid)?;
    commits.push(GitCommit::parse(id.to_owned(), content)?);
    stdout = stdout.get(header_end + 2 + size..).unwrap_or_default();
  }
  Ok(commits)
}

/// Failures only fetching a branch can have, falling back to [`classify_failure`].
fn classify_fetch_branch_failure(url: &str, branch: &str, stderr: &str) -> Option<GitError> {
  let lower = stderr.to_ascii_lowercase();
  // The first is git's own message; smart HTTP (e.g. GitHub) only reports the broken response.
  if lower.contains("no commits selected for shallow requests") || lower.contains("error processing shallow info") {
    Some(GitError::NoUniqueCommits { branch: branch.to_owned() })
  } else if lower.contains("couldn't find remote ref") {
    Some(GitError::BranchNotFound { branch: branch.to_owned() })
  } else {
    classify_failure(url, stderr)
  }
}

/// Failures only a push can have, falling back to [`classify_failure`].
///
/// git reports each refused ref as ` ! [rejected] <src> -> <dst> (<reason>)`, or `[remote
/// rejected]` when `origin` refused it, even with `--quiet`.
fn classify_push_failure(url: &str, branch: &str, stderr: &str) -> Option<GitError> {
  let rejected = stderr.lines().map(str::trim).find(|line| line.starts_with("! [") && line.contains("rejected]"));
  let Some(rejected) = rejected else {
    return classify_failure(url, stderr);
  };
  let reason = match (rejected.rfind('('), rejected.ends_with(')')) {
    (Some(open), true) => &rejected[open + 1..rejected.len() - 1],
    _ => rejected,
  };
  Some(if reason == "stale info" {
    GitError::StaleLease { branch: branch.to_owned() }
  } else {
    GitError::PushRejected { branch: branch.to_owned(), reason: reason.to_owned() }
  })
}

/// Failures only a clone can have, falling back to [`classify_failure`].
fn classify_clone_failure(options: &CloneOptions, stderr: &str) -> Option<GitError> {
  let lower = stderr.to_ascii_lowercase();
  if let Some(branch) = options.branch()
    && lower.contains("remote branch")
    && lower.contains("not found")
  {
    Some(GitError::BranchNotFound { branch: branch.to_owned() })
  } else if lower.contains("already exists and is not an empty directory") {
    Some(GitError::DestinationNotEmpty { path: options.destination().to_path_buf() })
  } else {
    classify_failure(options.url(), stderr)
  }
}

/// git exits 128 for nearly everything, so the cause of a failure reaching `url` has to be read from stderr.
fn classify_failure(url: &str, stderr: &str) -> Option<GitError> {
  let url = redact(url);
  let lower = stderr.to_ascii_lowercase();
  let has = |needles: &[&str]| needles.iter().any(|needle| lower.contains(needle));

  Some(
    if has(&[
      "authentication failed",
      "could not read username",
      "could not read password",
      "terminal prompts disabled",
      "invalid username or password",
      "permission denied (publickey",
      "returned error: 401",
      "returned error: 403",
    ]) {
      GitError::AuthenticationFailed { url }
    } else if has(&["repository not found", "does not appear to be a git repository", "returned error: 404"])
      || (has(&["fatal: repository '"]) && has(&["' not found"]))
    {
      GitError::RepositoryNotFound { url }
    } else if has(&[
      "could not resolve host",
      "connection refused",
      "timed out",
      "network is unreachable",
      "unable to access",
      "could not read from remote repository",
    ]) {
      GitError::RemoteUnreachable { url, reason: last_line(stderr) }
    } else {
      return None;
    },
  )
}

fn last_line(stderr: &str) -> String {
  let line = stderr.lines().map(str::trim).rfind(|line| !line.is_empty()).unwrap_or_default();
  line.strip_prefix("fatal: ").unwrap_or(line).to_owned()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::test_support::{Expect, backend_contract_tests};

  backend_contract_tests!(crate::GitCli);

  fn env<'a>(invocation: &'a Invocation, key: &str) -> Option<&'a str> {
    invocation.envs.iter().find(|(k, _)| *k == key).and_then(|(_, v)| v.to_str())
  }

  fn args(invocation: &Invocation) -> Vec<&str> {
    invocation.args.iter().map(|arg| arg.to_str().unwrap()).collect()
  }

  #[test]
  fn builds_a_plain_clone() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b");
    let invocation = clone_invocation(&options, None).unwrap();
    assert_eq!(args(&invocation), ["clone", "--quiet", "--", "https://github.com/a/b.git", "/tmp/b"]);
    assert_eq!(env(&invocation, "GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(env(&invocation, "GIT_CONFIG_COUNT"), None);
    assert_eq!(env(&invocation, "GIT_SSH_COMMAND"), None);
  }

  #[test]
  fn passes_branch_and_depth_as_single_args() {
    let options =
      CloneOptions::new("https://github.com/a/b.git", "/tmp/b").with_branch("-x").with_depth(3.try_into().unwrap());
    let invocation = clone_invocation(&options, None).unwrap();
    assert_eq!(
      args(&invocation),
      ["clone", "--quiet", "--branch=-x", "--depth=3", "--", "https://github.com/a/b.git", "/tmp/b"]
    );
  }

  #[test]
  fn https_auth_goes_in_an_extra_header_not_the_args() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b")
      .with_auth(Auth::Https { user: "user".into(), pass: "hunter2".into() });
    let invocation = clone_invocation(&options, None).unwrap();
    assert!(args(&invocation).iter().all(|arg| !arg.contains("hunter2")));
    assert_eq!(env(&invocation, "GIT_CONFIG_COUNT"), Some("1"));
    assert_eq!(env(&invocation, "GIT_CONFIG_KEY_0"), Some("http.https://github.com/.extraheader"));
    assert_eq!(env(&invocation, "GIT_CONFIG_VALUE_0"), Some(basic_auth_header("user", "hunter2").as_str()));
  }

  #[tokio::test]
  async fn ssh_auth_sets_the_ssh_command() {
    let key = crate::test_support::generate_ssh_key("ed25519", "");
    let agent = SshAgent::new(&key).unwrap();
    let options = CloneOptions::new("git@github.com:a/b.git", "/tmp/b").with_auth(Auth::Ssh { key });
    let invocation = clone_invocation(&options, Some(&agent)).unwrap();
    assert_eq!(env(&invocation, "GIT_SSH_COMMAND"), Some(agent.ssh_command().unwrap().as_str()));
    assert_eq!(env(&invocation, "GIT_SSH_VARIANT"), Some("ssh"));
    assert_eq!(env(&invocation, "GIT_CONFIG_COUNT"), None);
  }

  #[test]
  fn builds_ls_remote_against_origin_in_the_clone() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b")
      .with_auth(Auth::Https { user: "user".into(), pass: "hunter2".into() });
    let repo = ClonedRepo::new(crate::Git::new(GitCli), &options);
    let invocation = ls_remote_invocation(&repo, None).unwrap();
    assert_eq!(args(&invocation), ["ls-remote", "--heads", "origin"]);
    assert_eq!(invocation.current_dir.as_deref(), Some(std::path::Path::new("/tmp/b")));
    assert_eq!(env(&invocation, "GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(env(&invocation, "GIT_CONFIG_VALUE_0"), Some(basic_auth_header("user", "hunter2").as_str()));
  }

  #[test]
  fn parses_branch_names_from_ls_remote() {
    let stdout = "1111\trefs/heads/main\n2222\trefs/heads/feature/x\r\n3333\trefs/tags/v1\n\n";
    assert_eq!(parse_ls_remote(stdout), ["main", "feature/x"]);
  }

  fn https_repo() -> ClonedRepo {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b")
      .with_auth(Auth::Https { user: "user".into(), pass: "hunter2".into() });
    ClonedRepo::new(crate::Git::new(GitCli), &options)
  }

  #[test]
  fn builds_ls_remote_for_the_default_branch_and_a_branch() {
    let repo = https_repo();
    let invocation = ls_remote_tips_invocation(&repo, Some("doup/x"), None).unwrap();
    assert_eq!(args(&invocation), ["ls-remote", "--symref", "origin", "HEAD", "refs/heads/doup/x"]);
    assert_eq!(env(&invocation, "GIT_CONFIG_VALUE_0"), Some(basic_auth_header("user", "hunter2").as_str()));
    let invocation = ls_remote_tips_invocation(&repo, None, None).unwrap();
    assert_eq!(args(&invocation), ["ls-remote", "--symref", "origin", "HEAD"]);
  }

  #[test]
  fn builds_a_blobless_branch_fetch_that_excludes_the_default_branch() {
    let invocation = fetch_branch_invocation(&https_repo(), "-x", "main", None).unwrap();
    assert_eq!(
      args(&invocation),
      [
        "fetch",
        "--quiet",
        "--no-tags",
        "--filter=blob:none",
        "--shallow-exclude=refs/heads/main",
        "origin",
        "+refs/heads/-x:refs/remotes/origin/-x"
      ]
    );
    assert!(args(&invocation).iter().all(|arg| !arg.contains("hunter2")));
    assert_eq!(invocation.current_dir.as_deref(), Some(std::path::Path::new("/tmp/b")));
    assert_eq!(env(&invocation, "GIT_TERMINAL_PROMPT"), Some("0"));
  }

  #[test]
  fn parses_remote_tips() {
    let stdout = "ref: refs/heads/main\tHEAD\n1111\tHEAD\n2222\trefs/heads/x/HEAD\n3333\trefs/heads/x\r\n";
    let tips = parse_remote_tips(stdout, Some("x")).unwrap();
    assert_eq!((tips.default_branch.as_str(), tips.default_tip.as_str()), ("main", "1111"));
    assert_eq!(tips.branch_tip.as_deref(), Some("3333"));
    assert_eq!(parse_remote_tips(stdout, Some("y")).unwrap().branch_tip, None);
    assert_eq!(parse_remote_tips("1111\tHEAD\n", None), None, "detached HEAD");
    assert_eq!(parse_remote_tips("ref: refs/heads/main\tHEAD\n", None), None, "unborn HEAD");
  }

  #[test]
  fn parses_cat_file_batch_output() {
    let one = "tree t\nauthor A <a@x> 1 +0000\n\none\n";
    let two = "tree t\nparent p\nparent q\nauthor B <b@x> 1 +0000\n\ntwo";
    let stdout = format!("aaaa commit {}\n{one}\nbbbb commit {}\n{two}\n", one.len(), two.len());
    let commits = parse_cat_file_batch(stdout.as_bytes()).unwrap();
    assert_eq!(commits.iter().map(GitCommit::id).collect::<Vec<_>>(), ["aaaa", "bbbb"]);
    assert_eq!((commits[0].message(), commits[1].message()), ("one\n", "two"));
    assert!(commits[1].is_merge());

    for bad in ["aaaa missing\n", "aaaa blob 3\nabc\n", "aaaa commit 99\nshort\n"] {
      let err = parse_cat_file_batch(bad.as_bytes()).unwrap_err();
      assert!(matches!(&err, GitError::InvalidCommit { id } if id == "aaaa"), "{bad}: {err:?}");
    }
  }

  fn worktree() -> Worktree {
    Worktree::new(https_repo(), "doup/x", None).unwrap()
  }

  #[test]
  fn worktree_commands_disable_hooks_and_commit_as_the_identity() {
    let worktree = worktree();
    let hooks = format!("core.hooksPath={}", worktree.hooks_path().display());
    let invocation = commit_invocation(&worktree, "-m msg");
    assert_eq!(
      args(&invocation),
      ["-c", &hooks, "-c", "commit.gpgsign=false", "commit", "--quiet", "--no-verify", "--message=-m msg"]
    );
    assert_eq!(invocation.current_dir.as_deref(), Some(worktree.path()));
    assert_eq!(env(&invocation, "GIT_AUTHOR_EMAIL"), Some("doup[bot]@users.noreply.github.com"));
    assert_eq!(env(&invocation, "GIT_COMMITTER_NAME"), Some("doup[bot]"));

    let invocation = worktree_add_invocation(&worktree);
    let path = worktree.path().to_str().unwrap();
    assert_eq!(
      args(&invocation),
      ["-c", &hooks, "worktree", "add", "--quiet", "--no-track", "-B", "doup/x", path, "HEAD"]
    );
    assert_eq!(invocation.current_dir.as_deref(), Some(std::path::Path::new("/tmp/b")));
  }

  #[test]
  fn builds_pushes_of_an_exact_id_with_an_optional_lease() {
    let worktree = worktree();
    let push = |mode| {
      let invocation = push_invocation(&worktree, "1111", mode, None).unwrap();
      assert!(args(&invocation).iter().all(|arg| !arg.contains("hunter2")));
      assert_eq!(env(&invocation, "GIT_CONFIG_VALUE_0"), Some(basic_auth_header("user", "hunter2").as_str()));
      args(&invocation)[2..].iter().map(|arg| arg.to_string()).collect::<Vec<_>>()
    };
    assert_eq!(push(Push::FastForward), ["push", "--quiet", "--no-verify", "origin", "1111:refs/heads/doup/x"]);
    assert_eq!(
      push(Push::ForceWithLease(Some("2222"))),
      [
        "push",
        "--quiet",
        "--no-verify",
        "--force-with-lease=refs/heads/doup/x:2222",
        "origin",
        "1111:refs/heads/doup/x"
      ]
    );
    assert_eq!(push(Push::ForceWithLease(None))[3], "--force-with-lease=refs/heads/doup/x:");
  }

  #[test]
  fn builds_a_leased_branch_delete_in_the_clone() {
    let invocation = delete_invocation(&https_repo(), "doup/x", "2222", None).unwrap();
    assert_eq!(
      args(&invocation),
      ["push", "--quiet", "--no-verify", "--force-with-lease=refs/heads/doup/x:2222", "origin", ":refs/heads/doup/x"]
    );
    assert_eq!(invocation.current_dir.as_deref(), Some(std::path::Path::new("/tmp/b")));
    assert_eq!(env(&invocation, "GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(env(&invocation, "GIT_CONFIG_VALUE_0"), Some(basic_auth_header("user", "hunter2").as_str()));
  }

  #[test]
  fn classifies_push_failures() {
    let url = "https://u:secret@github.com/a/b.git";
    let cases: &[(&str, Expect)] = &[
      (
        " ! [rejected]        1111 -> doup/x (stale info)\nerror: failed to push some refs",
        |e| matches!(e, GitError::StaleLease { branch } if branch == "doup/x"),
      ),
      (
        " ! [rejected]        1111 -> doup/x (fetch first)\nerror: failed to push some refs",
        |e| matches!(e, GitError::PushRejected { reason, .. } if reason == "fetch first"),
      ),
      (
        " ! [remote rejected] 1111 -> doup/x (protected branch hook declined)\n",
        |e| matches!(e, GitError::PushRejected { reason, .. } if reason == "protected branch hook declined"),
      ),
      ("fatal: Authentication failed for 'https://github.com/a/b.git/'", |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
    ];
    for (stderr, expected) in cases {
      let err = classify_push_failure(url, "doup/x", stderr).unwrap();
      assert!(expected(&err), "{stderr} => {err:?}");
      assert!(!err.to_string().contains("secret"), "{err}");
    }
    assert!(classify_push_failure(url, "doup/x", "fatal: something else").is_none());
  }

  #[test]
  fn classifies_branch_fetch_failures() {
    let url = "https://u:secret@github.com/a/b.git";
    let cases: &[(&str, Expect)] = &[
      (
        "fatal: no commits selected for shallow requests\nfatal: the remote end hung up unexpectedly",
        |e| matches!(e, GitError::NoUniqueCommits { branch } if branch == "x"),
      ),
      ("fatal: error processing shallow info: 4", |e| matches!(e, GitError::NoUniqueCommits { .. })),
      (
        "fatal: couldn't find remote ref refs/heads/x",
        |e| matches!(e, GitError::BranchNotFound { branch } if branch == "x"),
      ),
      ("fatal: Authentication failed for 'https://github.com/a/b.git/'", |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
    ];
    for (stderr, expected) in cases {
      let err = classify_fetch_branch_failure(url, "x", stderr).unwrap();
      assert!(expected(&err), "{stderr} => {err:?}");
      assert!(!err.to_string().contains("secret"), "{err}");
    }
  }

  #[test]
  fn classifies_git_failures() {
    let plain = CloneOptions::new("https://u:secret@github.com/a/b.git", "/tmp/b");
    let branch = plain.clone().with_branch("nope");
    let cases: &[(&CloneOptions, &str, Expect)] = &[
      (&plain, "fatal: Authentication failed for 'https://github.com/a/b.git/'", |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
      (&plain, "fatal: could not read Username for 'https://github.com': terminal prompts disabled", |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
      (&plain, "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.", |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
      (
        &branch,
        "warning: Could not find remote branch nope to clone.\nfatal: Remote branch nope not found in upstream origin",
        |e| matches!(e, GitError::BranchNotFound { branch } if branch == "nope"),
      ),
      (&plain, "fatal: destination path '/tmp/b' already exists and is not an empty directory.", |e| {
        matches!(e, GitError::DestinationNotEmpty { .. })
      }),
      (&plain, "fatal: repository 'https://github.com/a/b.git/' not found", |e| {
        matches!(e, GitError::RepositoryNotFound { .. })
      }),
      (&plain, "fatal: '/srv/missing.git' does not appear to be a git repository", |e| {
        matches!(e, GitError::RepositoryNotFound { .. })
      }),
      (
        &plain,
        "fatal: unable to access 'https://nope.invalid/a.git/': Could not resolve host: nope.invalid",
        |e| matches!(e, GitError::RemoteUnreachable { reason, .. } if reason.starts_with("unable to access")),
      ),
    ];
    for (options, stderr, expected) in cases {
      let err = classify_clone_failure(options, stderr).unwrap();
      assert!(expected(&err), "{stderr} => {err:?}");
      assert!(!err.to_string().contains("secret"), "{err}");
    }
  }

  #[test]
  fn leaves_unrecognised_failures_unclassified() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b");
    assert!(classify_clone_failure(&options, "fatal: something else").is_none());
  }
}

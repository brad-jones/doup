use crate::auth::basic_auth_header;
use crate::remote_url::{RemoteUrl, redact};
use crate::ssh_agent::SshAgent;
use crate::{Auth, CloneOptions, GitBackend, GitError};
use std::ffi::OsString;
use std::io::ErrorKind;
use std::process::Stdio;
use tokio::process::Command;

/// Shells out to the `git` executable found on `PATH`.
#[derive(Debug, Clone, Copy, Default)]
pub struct GitCli;

impl GitBackend for GitCli {
  async fn clone_repo(&self, options: &CloneOptions) -> Result<(), GitError> {
    // Must outlive the child process, which signs through the agent.
    let agent = SshAgent::for_auth(options.auth())?;
    let invocation = clone_invocation(options, agent.as_ref())?;
    let output = Command::new("git")
      .args(&invocation.args)
      .envs(invocation.envs)
      .stdin(Stdio::null())
      .stdout(Stdio::null())
      .stderr(Stdio::piped())
      .kill_on_drop(true)
      .output()
      .await
      .map_err(|err| match err.kind() {
        ErrorKind::NotFound => GitError::GitNotInstalled,
        _ => GitError::Io(err),
      })?;
    drop(agent);

    if output.status.success() {
      return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(
      classify_failure(options, &stderr).unwrap_or_else(|| GitError::CommandFailed {
        status: output.status,
        stderr: stderr.trim().to_owned(),
      }),
    )
  }
}

#[derive(Debug)]
struct Invocation {
  args: Vec<OsString>,
  envs: Vec<(&'static str, OsString)>,
}

/// Secrets only ever go in the environment, never in args that `ps` can show.
fn clone_invocation(
  options: &CloneOptions,
  agent: Option<&SshAgent>,
) -> Result<Invocation, GitError> {
  let mut args: Vec<OsString> = vec!["clone".into(), "--quiet".into()];
  if let Some(branch) = options.branch() {
    args.push(format!("--branch={branch}").into());
  }
  if let Some(depth) = options.depth() {
    args.push(format!("--depth={depth}").into());
  }
  args.extend([
    "--".into(),
    options.url().into(),
    options.destination().into(),
  ]);

  let mut envs = vec![("GIT_TERMINAL_PROMPT", "0".into())];
  if let Some(Auth::Https { user, pass }) = options.auth() {
    let key = RemoteUrl::parse(options.url())?
      .extra_header_key()
      .ok_or_else(|| GitError::InvalidUrl {
        url: redact(options.url()),
      })?;
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
  Ok(Invocation { args, envs })
}

/// git exits 128 for nearly everything, so the cause has to be read from stderr.
fn classify_failure(options: &CloneOptions, stderr: &str) -> Option<GitError> {
  let url = redact(options.url());
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
    } else if let Some(branch) = options.branch()
      && has(&["remote branch"])
      && has(&["not found"])
    {
      GitError::BranchNotFound {
        branch: branch.to_owned(),
      }
    } else if has(&["already exists and is not an empty directory"]) {
      GitError::DestinationNotEmpty {
        path: options.destination().to_path_buf(),
      }
    } else if has(&[
      "repository not found",
      "does not appear to be a git repository",
      "returned error: 404",
    ]) || (has(&["fatal: repository '"]) && has(&["' not found"]))
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
      GitError::RemoteUnreachable {
        url,
        reason: last_line(stderr),
      }
    } else {
      return None;
    },
  )
}

fn last_line(stderr: &str) -> String {
  let line = stderr
    .lines()
    .map(str::trim)
    .rfind(|line| !line.is_empty())
    .unwrap_or_default();
  line.strip_prefix("fatal: ").unwrap_or(line).to_owned()
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::test_support::{Expect, backend_contract_tests};

  backend_contract_tests!(crate::GitCli);

  fn env<'a>(invocation: &'a Invocation, key: &str) -> Option<&'a str> {
    invocation
      .envs
      .iter()
      .find(|(k, _)| *k == key)
      .and_then(|(_, v)| v.to_str())
  }

  fn args(invocation: &Invocation) -> Vec<&str> {
    invocation
      .args
      .iter()
      .map(|arg| arg.to_str().unwrap())
      .collect()
  }

  #[test]
  fn builds_a_plain_clone() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b");
    let invocation = clone_invocation(&options, None).unwrap();
    assert_eq!(
      args(&invocation),
      [
        "clone",
        "--quiet",
        "--",
        "https://github.com/a/b.git",
        "/tmp/b"
      ]
    );
    assert_eq!(env(&invocation, "GIT_TERMINAL_PROMPT"), Some("0"));
    assert_eq!(env(&invocation, "GIT_CONFIG_COUNT"), None);
    assert_eq!(env(&invocation, "GIT_SSH_COMMAND"), None);
  }

  #[test]
  fn passes_branch_and_depth_as_single_args() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b")
      .with_branch("-x")
      .with_depth(3.try_into().unwrap());
    let invocation = clone_invocation(&options, None).unwrap();
    assert_eq!(
      args(&invocation),
      [
        "clone",
        "--quiet",
        "--branch=-x",
        "--depth=3",
        "--",
        "https://github.com/a/b.git",
        "/tmp/b"
      ]
    );
  }

  #[test]
  fn https_auth_goes_in_an_extra_header_not_the_args() {
    let options =
      CloneOptions::new("https://github.com/a/b.git", "/tmp/b").with_auth(Auth::Https {
        user: "user".into(),
        pass: "hunter2".into(),
      });
    let invocation = clone_invocation(&options, None).unwrap();
    assert!(args(&invocation).iter().all(|arg| !arg.contains("hunter2")));
    assert_eq!(env(&invocation, "GIT_CONFIG_COUNT"), Some("1"));
    assert_eq!(
      env(&invocation, "GIT_CONFIG_KEY_0"),
      Some("http.https://github.com/.extraheader")
    );
    assert_eq!(
      env(&invocation, "GIT_CONFIG_VALUE_0"),
      Some(basic_auth_header("user", "hunter2").as_str())
    );
  }

  #[tokio::test]
  async fn ssh_auth_sets_the_ssh_command() {
    let key = crate::test_support::generate_ssh_key("ed25519", "");
    let agent = SshAgent::new(&key).unwrap();
    let options =
      CloneOptions::new("git@github.com:a/b.git", "/tmp/b").with_auth(Auth::Ssh { key });
    let invocation = clone_invocation(&options, Some(&agent)).unwrap();
    assert_eq!(
      env(&invocation, "GIT_SSH_COMMAND"),
      Some(agent.ssh_command().unwrap().as_str())
    );
    assert_eq!(env(&invocation, "GIT_SSH_VARIANT"), Some("ssh"));
    assert_eq!(env(&invocation, "GIT_CONFIG_COUNT"), None);
  }

  #[test]
  fn classifies_git_failures() {
    let plain = CloneOptions::new("https://u:secret@github.com/a/b.git", "/tmp/b");
    let branch = plain.clone().with_branch("nope");
    let cases: &[(&CloneOptions, &str, Expect)] = &[
      (
        &plain,
        "fatal: Authentication failed for 'https://github.com/a/b.git/'",
        |e| matches!(e, GitError::AuthenticationFailed { .. }),
      ),
      (
        &plain,
        "fatal: could not read Username for 'https://github.com': terminal prompts disabled",
        |e| matches!(e, GitError::AuthenticationFailed { .. }),
      ),
      (
        &plain,
        "git@github.com: Permission denied (publickey).\nfatal: Could not read from remote repository.",
        |e| matches!(e, GitError::AuthenticationFailed { .. }),
      ),
      (
        &branch,
        "warning: Could not find remote branch nope to clone.\nfatal: Remote branch nope not found in upstream origin",
        |e| matches!(e, GitError::BranchNotFound { branch } if branch == "nope"),
      ),
      (
        &plain,
        "fatal: destination path '/tmp/b' already exists and is not an empty directory.",
        |e| matches!(e, GitError::DestinationNotEmpty { .. }),
      ),
      (
        &plain,
        "fatal: repository 'https://github.com/a/b.git/' not found",
        |e| matches!(e, GitError::RepositoryNotFound { .. }),
      ),
      (
        &plain,
        "fatal: '/srv/missing.git' does not appear to be a git repository",
        |e| matches!(e, GitError::RepositoryNotFound { .. }),
      ),
      (
        &plain,
        "fatal: unable to access 'https://nope.invalid/a.git/': Could not resolve host: nope.invalid",
        |e| matches!(e, GitError::RemoteUnreachable { reason, .. } if reason.starts_with("unable to access")),
      ),
    ];
    for (options, stderr, expected) in cases {
      let err = classify_failure(options, stderr).unwrap();
      assert!(expected(&err), "{stderr} => {err:?}");
      assert!(!err.to_string().contains("secret"), "{err}");
    }
  }

  #[test]
  fn leaves_unrecognised_failures_unclassified() {
    let options = CloneOptions::new("https://github.com/a/b.git", "/tmp/b");
    assert!(classify_failure(&options, "fatal: something else").is_none());
  }
}

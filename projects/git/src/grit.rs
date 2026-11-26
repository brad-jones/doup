use crate::remote_url::{RemoteUrl, redact};
use crate::ssh_agent::SshAgent;
use crate::{Auth, CloneOptions, ClonedRepo, GitBackend, GitCommit, GitError, RemoteTips};
use grit_lib::config::ConfigSet;
use grit_lib::config::environment::{Environment, RepositoryOptions};
use grit_lib::error::{Error, RemoteUnreadableReason};
use grit_lib::objects::ObjectKind;
use grit_lib::progress::{CancelToken, NullProgress};
use grit_lib::remote::clone::{self as grit_clone, CloneError};
use grit_lib::remote::credentials::{Credential, CredentialProvider};
use grit_lib::remote::fetch::NoProgress;
use grit_lib::remote::fetch_request::FetchRefspecError;
use grit_lib::remote::http_client::{HttpClient, UreqHttpClient};
use grit_lib::remote::partial_clone::ObjectFilterSpec;
use grit_lib::remote::transfer::{FetchOptions, TagMode};
use grit_lib::remote::transport_path::resolve_local_remote_git_dir;
use grit_lib::remote::{
  DEFAULT_REMOTE, HttpClientFactory, ListRefsOptions, Remote, RemoteError, RemoteRef, RemoteUrl as GritRemoteUrl,
};
use grit_lib::repo::Repository;
use grit_lib::revwalk::RevWalkListOptions;
use std::num::NonZeroU32;
use url::Url;

/// Performs Git operations in-process with grit, without needing a `git` executable.
///
/// SSH remotes still need an `ssh` executable, as grit shells out for that transport.
#[derive(Debug, Clone, Copy, Default)]
pub struct Grit;

impl GitBackend for Grit {
  async fn clone_repo(&self, options: &CloneOptions) -> Result<(), GitError> {
    // Held here, not in the blocking task, so it stops if this future is dropped.
    let agent = SshAgent::for_auth(options.auth())?;
    let ssh_command = agent.as_ref().map(SshAgent::ssh_command).transpose()?;
    let options = options.clone();
    let cancel = CancelToken::new();
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let result = tokio::task::spawn_blocking(move || clone_blocking(&options, ssh_command, &cancel)).await?;
    drop(agent);
    result
  }

  async fn list_remote_branches(&self, repo: &ClonedRepo) -> Result<Vec<String>, GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let ssh_command = agent.as_ref().map(SshAgent::ssh_command).transpose()?;
    let repo = repo.clone();
    let result = tokio::task::spawn_blocking(move || list_remote_branches_blocking(&repo, ssh_command)).await?;
    drop(agent);
    result
  }

  async fn remote_tips(&self, repo: &ClonedRepo, branch: Option<&str>) -> Result<RemoteTips, GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let ssh_command = agent.as_ref().map(SshAgent::ssh_command).transpose()?;
    let (repo, branch) = (repo.clone(), branch.map(str::to_owned));
    let result =
      tokio::task::spawn_blocking(move || remote_tips_blocking(&repo, branch.as_deref(), ssh_command)).await?;
    drop(agent);
    result
  }

  async fn fetch_branch(&self, repo: &ClonedRepo, branch: &str, exclude: &str) -> Result<(), GitError> {
    let agent = SshAgent::for_auth(repo.auth())?;
    let ssh_command = agent.as_ref().map(SshAgent::ssh_command).transpose()?;
    let (repo, branch, exclude) = (repo.clone(), branch.to_owned(), exclude.to_owned());
    let cancel = CancelToken::new();
    let _cancel_on_drop = CancelOnDrop(cancel.clone());
    let result =
      tokio::task::spawn_blocking(move || fetch_branch_blocking(&repo, &branch, &exclude, ssh_command, &cancel))
        .await?;
    drop(agent);
    result
  }

  async fn branch_commits(&self, repo: &ClonedRepo, branch: &str) -> Result<Vec<GitCommit>, GitError> {
    let (repo, branch) = (repo.clone(), branch.to_owned());
    tokio::task::spawn_blocking(move || branch_commits_blocking(&repo, &branch)).await?
  }
}

/// grit is synchronous, so stop its blocking work if the awaiting future is dropped.
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
  fn drop(&mut self) {
    self.0.cancel();
  }
}

/// grit hands `GIT_SSH_COMMAND` to `sh -c` when it spawns ssh, so the agent's command goes there.
fn with_ssh_command(environment: Environment, ssh_command: Option<String>) -> Environment {
  match ssh_command {
    Some(ssh_command) => environment.with_var("GIT_SSH_COMMAND", ssh_command).with_var("GIT_SSH_VARIANT", "ssh"),
    None => environment,
  }
}

fn http_factory(
  url: &str,
  auth: Option<&Auth>,
  environment: &Environment,
) -> Result<Option<StaticCredentialsFactory>, GitError> {
  Ok(match (auth, RemoteUrl::parse(url)?) {
    (Some(Auth::Https { user, pass }), RemoteUrl::Http(url)) => Some(StaticCredentialsFactory {
      credentials: StaticCredentials::new(&url, user, pass),
      environment: environment.clone(),
    }),
    _ => None,
  })
}

fn clone_blocking(options: &CloneOptions, ssh_command: Option<String>, cancel: &CancelToken) -> Result<(), GitError> {
  let dest = options.destination();
  let dest_existed = dest.exists();

  let mut grit_options = grit_clone::CloneOptions::new(options.url(), dest.to_path_buf());
  grit_options.branch = options.branch().map(str::to_owned);
  grit_options.depth = options.depth().map(NonZeroU32::get);
  grit_options.environment = with_ssh_command(grit_options.environment, ssh_command);
  let http_factory = http_factory(options.url(), options.auth(), &grit_options.environment)?;

  let result = grit_clone::clone(
    grit_options,
    &mut NullProgress,
    cancel,
    http_factory.as_ref().map(|factory| factory as &dyn HttpClientFactory),
  );

  match result {
    Ok(_) => Ok(()),
    Err(err) => {
      if grit_clone::should_cleanup_failed_clone(&err) {
        grit_clone::cleanup_failed_clone(dest, dest_existed);
      }
      Err(classify_clone_failure(options, &err).unwrap_or(GitError::Grit(err)))
    }
  }
}

fn list_remote_branches_blocking(repo: &ClonedRepo, ssh_command: Option<String>) -> Result<Vec<String>, GitError> {
  let list = ListRefsOptions { heads: true, ..ListRefsOptions::default() };
  let refs = ls_remote(repo, ssh_command, &list)?;
  Ok(refs.into_iter().filter_map(|r| r.name.strip_prefix("refs/heads/").map(str::to_owned)).collect())
}

fn remote_tips_blocking(
  repo: &ClonedRepo,
  branch: Option<&str>,
  ssh_command: Option<String>,
) -> Result<RemoteTips, GitError> {
  let branch_ref = branch.map(|branch| format!("refs/heads/{branch}"));
  let prefixes = std::iter::once("HEAD".to_owned()).chain(branch_ref.clone()).collect();
  let list = ListRefsOptions { symrefs: true, prefixes, ..ListRefsOptions::default() };
  let refs = ls_remote(repo, ssh_command, &list)?;

  let head = refs.iter().find(|r| r.name == "HEAD");
  let default_branch = head.and_then(|r| r.symref_target.as_deref()?.strip_prefix("refs/heads/"));
  let (Some(head), Some(default_branch)) = (head, default_branch) else {
    return Err(GitError::NoDefaultBranch { url: redact(repo.url()) });
  };
  Ok(RemoteTips {
    default_branch: default_branch.to_owned(),
    default_tip: head.oid.to_hex(),
    branch_tip: refs.iter().find(|r| Some(&r.name) == branch_ref.as_ref()).map(|r| r.oid.to_hex()),
  })
}

/// Lists refs on `repo`'s `origin`, like `git ls-remote origin`.
fn ls_remote(
  repo: &ClonedRepo,
  ssh_command: Option<String>,
  list: &ListRefsOptions,
) -> Result<Vec<RemoteRef>, GitError> {
  let (git_repo, http_factory) = open(repo, ssh_command)?;
  let refs = git_repo.remotes().get(DEFAULT_REMOTE).and_then(|remote| {
    git_repo.remotes().ls_remote(&remote, list, http_factory.as_ref().map(|factory| factory as &dyn HttpClientFactory))
  });
  refs.map_err(|err| classify_failure(repo.url(), &err).unwrap_or(GitError::Grit(err)))
}

fn fetch_branch_blocking(
  repo: &ClonedRepo,
  branch: &str,
  exclude: &str,
  ssh_command: Option<String>,
  cancel: &CancelToken,
) -> Result<(), GitError> {
  let (git_repo, http_factory) = open(repo, ssh_command)?;
  let mut options = FetchOptions::default();
  options.refspecs = vec![format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")];
  options.tags = TagMode::None;
  options.deepen_not = vec![format!("refs/heads/{exclude}")];
  options.filter = Some(ObjectFilterSpec::BlobNone);
  options.environment = git_repo.environment().clone();

  let fetched = git_repo.remotes().get(DEFAULT_REMOTE).and_then(|remote| {
    if is_local(&remote) && !has_unique_commits(&git_repo, repo, branch, exclude)? {
      return Ok(None);
    }
    let http_factory = http_factory.as_ref().map(|factory| factory as &dyn HttpClientFactory);
    git_repo.remotes().fetch(&remote, options, &mut NoProgress, cancel, http_factory).map(Some)
  });
  match fetched {
    Ok(Some(_)) => Ok(()),
    Ok(None) => Err(GitError::NoUniqueCommits { branch: branch.to_owned() }),
    Err(err) => Err(classify_fetch_branch_failure(repo.url(), branch, &err).unwrap_or(GitError::Grit(err))),
  }
}

fn is_local(remote: &Remote) -> bool {
  matches!(remote.fetch_url(), GritRemoteUrl::Local(_) | GritRemoteUrl::File(_))
}

/// Whether the local remote's `branch` has commits that are not on its `exclude` branch.
///
/// git's upload-pack refuses a deepen-not fetch that would send nothing, but grit's in-process
/// one, serving local remotes, sends the whole history instead. So that case is caught first.
fn has_unique_commits(
  git_repo: &Repository,
  repo: &ClonedRepo,
  branch: &str,
  exclude: &str,
) -> grit_lib::error::Result<bool> {
  let git_dir = resolve_local_remote_git_dir(repo.url(), git_repo.git_dir(), Some(repo.path()));
  let remote_repo = Repository::open(&git_dir, &RepositoryOptions::with_environment(git_repo.environment().clone()))?;
  let (tip, exclude) = (format!("refs/heads/{branch}"), format!("^refs/heads/{exclude}"));
  Ok(remote_repo.revwalk().count(&[&tip, &exclude], &RevWalkListOptions::default())? > 0)
}

fn branch_commits_blocking(repo: &ClonedRepo, branch: &str) -> Result<Vec<GitCommit>, GitError> {
  let (git_repo, _) = open(repo, None)?;
  let tip = format!("refs/remotes/origin/{branch}");
  let ids = git_repo.revwalk().commits(&[&tip], &RevWalkListOptions::default()).map_err(GitError::Grit)?;
  ids
    .iter()
    .map(|id| {
      // Raw objects, as walks hide the parents of commits on a shallow boundary.
      let object = git_repo.objects().read(id).map_err(GitError::Grit)?;
      if object.kind != ObjectKind::Commit {
        return Err(GitError::InvalidCommit { id: id.to_hex() });
      }
      GitCommit::parse(id.to_hex(), &object.data)
    })
    .collect()
}

/// Opens the clone, with credentials ready for any request it makes to `origin`.
fn open(
  repo: &ClonedRepo,
  ssh_command: Option<String>,
) -> Result<(Repository, Option<StaticCredentialsFactory>), GitError> {
  // hygiene: embedder boundary, so the user's own Git setup applies like it does for a clone.
  let environment = with_ssh_command(Environment::capture_process(), ssh_command);
  let http_factory = http_factory(repo.url(), repo.auth(), &environment)?;
  let options = RepositoryOptions::with_environment(environment).with_work_tree(repo.path());
  let git_repo = Repository::open(&repo.path().join(".git"), &options).map_err(GitError::Grit)?;
  Ok((git_repo, http_factory))
}

/// Failures only a clone can have, falling back to [`classify_failure`].
fn classify_clone_failure(options: &CloneOptions, err: &Error) -> Option<GitError> {
  if let Error::Remote(RemoteError::Clone(clone)) = err {
    return match clone.as_ref() {
      CloneError::DestNotEmpty { .. } => {
        Some(GitError::DestinationNotEmpty { path: options.destination().to_path_buf() })
      }
      CloneError::RemoteRefNotFound { name } => {
        Some(GitError::BranchNotFound { branch: options.branch().unwrap_or(name).to_owned() })
      }
      _ => None,
    };
  }
  classify_failure(options.url(), err)
}

/// Failures only fetching a branch can have, falling back to [`classify_failure`].
fn classify_fetch_branch_failure(url: &str, branch: &str, err: &Error) -> Option<GitError> {
  match err {
    // git's upload-pack refuses a deepen-not that leaves nothing to send.
    Error::Remote(RemoteError::Fatal { message }) if message.contains("no commits selected for shallow requests") => {
      Some(GitError::NoUniqueCommits { branch: branch.to_owned() })
    }
    Error::Remote(RemoteError::FetchRefspec(FetchRefspecError::SourceNotFound { .. })) => {
      Some(GitError::BranchNotFound { branch: branch.to_owned() })
    }
    _ => classify_failure(url, err),
  }
}

fn classify_failure(url: &str, err: &Error) -> Option<GitError> {
  let url = redact(url);
  let Error::Remote(err) = err else {
    return None;
  };
  Some(match err {
    RemoteError::Auth { .. } | RemoteError::CredentialsUnavailable { .. } => GitError::AuthenticationFailed { url },
    RemoteError::HttpStatus { status: 401 | 403, .. } => GitError::AuthenticationFailed { url },
    RemoteError::HttpStatus { status: 404, .. } => GitError::RepositoryNotFound { url },
    RemoteError::Unreadable { reason: RemoteUnreadableReason::RepositoryNotFound, .. } => {
      GitError::RepositoryNotFound { url }
    }
    RemoteError::Unreadable { reason: RemoteUnreadableReason::HttpStatus { status: 404, .. }, .. } => {
      GitError::RepositoryNotFound { url }
    }
    RemoteError::Unreadable { .. } | RemoteError::Transport { .. } => {
      GitError::RemoteUnreachable { url, reason: err.to_string() }
    }
    _ => return None,
  })
}

/// Answers HTTP auth challenges from memory, so credentials never touch disk or a helper.
#[derive(Clone)]
struct StaticCredentials {
  protocol: String,
  host: String,
  user: String,
  pass: String,
}

impl StaticCredentials {
  fn new(url: &Url, user: &str, pass: &str) -> Self {
    let host = url.host_str().unwrap_or_default();
    let host = match url.port() {
      Some(port) => format!("{host}:{port}"),
      None => host.to_owned(),
    };
    Self { protocol: url.scheme().to_owned(), host, user: user.to_owned(), pass: pass.to_owned() }
  }
}

impl CredentialProvider for StaticCredentials {
  fn fill(&self, input: &Credential) -> grit_lib::error::Result<Credential> {
    let mut filled = input.clone();
    // Never hand the credentials to another origin, e.g. after a redirect.
    if input.protocol.as_deref() == Some(&self.protocol) && input.host.as_deref() == Some(&self.host) {
      filled.username = Some(self.user.clone());
      filled.password = Some(self.pass.clone());
    }
    Ok(filled)
  }

  fn approve(&self, _: &Credential) -> grit_lib::error::Result<()> {
    Ok(())
  }

  fn reject(&self, _: &Credential) -> grit_lib::error::Result<()> {
    Ok(())
  }
}

struct StaticCredentialsFactory {
  credentials: StaticCredentials,
  environment: Environment,
}

impl HttpClientFactory for StaticCredentialsFactory {
  fn create(&self, config: &ConfigSet) -> grit_lib::error::Result<Box<dyn HttpClient>> {
    let client = UreqHttpClient::from_config(config, &self.environment)?
      .with_credential_provider(Box::new(self.credentials.clone()));
    Ok(Box::new(client))
  }

  fn create_for_preflight(&self, config: &ConfigSet) -> grit_lib::error::Result<Box<dyn HttpClient>> {
    self.create(config)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::test_support::{Expect, backend_contract_tests};
  use grit_lib::error::AuthFailure;

  backend_contract_tests!(crate::Grit);

  fn credentials() -> StaticCredentials {
    let url = Url::parse("https://example.com:8443/a/b.git").unwrap();
    StaticCredentials::new(&url, "user", "hunter2")
  }

  fn request(protocol: &str, host: &str) -> Credential {
    Credential { protocol: Some(protocol.into()), host: Some(host.into()), ..Credential::default() }
  }

  #[test]
  fn fills_credentials_for_the_same_origin() {
    let filled = credentials().fill(&request("https", "example.com:8443")).unwrap();
    assert_eq!(filled.username.as_deref(), Some("user"));
    assert_eq!(filled.password.as_deref(), Some("hunter2"));
  }

  #[test]
  fn withholds_credentials_from_other_origins() {
    for (protocol, host) in [("https", "evil.example.com"), ("https", "example.com"), ("http", "example.com:8443")] {
      let filled = credentials().fill(&request(protocol, host)).unwrap();
      assert_eq!(filled.password, None, "{protocol}://{host}");
    }
  }

  #[test]
  fn factory_builds_a_client() {
    let factory = StaticCredentialsFactory { credentials: credentials(), environment: Environment::empty() };
    let config = ConfigSet::default();
    assert!(factory.create(&config).is_ok());
    assert!(factory.create_for_preflight(&config).is_ok());
  }

  #[test]
  fn classifies_grit_failures() {
    let options = CloneOptions::new("https://u:secret@example.com/a.git", "/tmp/b").with_branch("nope");
    let remote = |err: RemoteError| Error::Remote(err);
    let cases: Vec<(Error, Expect)> = vec![
      (remote(RemoteError::Auth { url: "https://example.com/a.git".into(), reason: AuthFailure::Rejected }), |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
      (remote(RemoteError::HttpStatus { url: None, status: 403 }), |e| {
        matches!(e, GitError::AuthenticationFailed { .. })
      }),
      (remote(RemoteError::Unreadable { url: None, reason: RemoteUnreadableReason::RepositoryNotFound }), |e| {
        matches!(e, GitError::RepositoryNotFound { .. })
      }),
      (
        remote(RemoteError::Unreadable { url: None, reason: RemoteUnreadableReason::HttpStatus { status: 404 } }),
        |e| matches!(e, GitError::RepositoryNotFound { .. }),
      ),
      (remote(RemoteError::Unreadable { url: None, reason: RemoteUnreadableReason::NoHandshake }), |e| {
        matches!(e, GitError::RemoteUnreachable { .. })
      }),
      (
        remote(RemoteError::Clone(Box::new(CloneError::RemoteRefNotFound { name: "nope".into() }))),
        |e| matches!(e, GitError::BranchNotFound { branch } if branch == "nope"),
      ),
      (remote(RemoteError::Clone(Box::new(CloneError::DestNotEmpty { path: "/tmp/b".into() }))), |e| {
        matches!(e, GitError::DestinationNotEmpty { .. })
      }),
    ];
    for (err, expected) in &cases {
      let classified = classify_clone_failure(&options, err).unwrap();
      assert!(expected(&classified), "{err:?} => {classified:?}");
      assert!(!classified.to_string().contains("secret"), "{classified}");
    }
  }

  #[test]
  fn leaves_other_failures_unclassified() {
    let options = CloneOptions::new("https://example.com/a.git", "/tmp/b");
    assert!(classify_clone_failure(&options, &Error::Cancelled).is_none());
  }

  #[test]
  fn classifies_branch_fetch_failures() {
    let fatal = |message: &str| Error::Remote(RemoteError::Fatal { message: message.into() });
    let missing = Error::Remote(RemoteError::FetchRefspec(FetchRefspecError::SourceNotFound {
      ref_name: "refs/heads/x".into(),
      spec: "+refs/heads/x:refs/remotes/origin/x".into(),
    }));
    let url = "https://example.com/a.git";
    let classify = |err: &Error| classify_fetch_branch_failure(url, "x", err);
    assert!(matches!(
      classify(&fatal("upload-pack: no commits selected for shallow requests")),
      Some(GitError::NoUniqueCommits { branch }) if branch == "x"
    ));
    assert!(matches!(classify(&missing), Some(GitError::BranchNotFound { branch }) if branch == "x"));
    assert!(classify(&fatal("upload-pack: not our ref")).is_none());
  }
}

use crate::remote_url::{RemoteUrl, redact};
use crate::ssh_agent::SshAgent;
use crate::{Auth, CloneOptions, GitBackend, GitError};
use grit_lib::config::ConfigSet;
use grit_lib::config::environment::Environment;
use grit_lib::error::{Error, RemoteUnreadableReason};
use grit_lib::progress::{CancelToken, NullProgress};
use grit_lib::remote::clone::{self as grit_clone, CloneError};
use grit_lib::remote::credentials::{Credential, CredentialProvider};
use grit_lib::remote::http_client::{HttpClient, UreqHttpClient};
use grit_lib::remote::{HttpClientFactory, RemoteError};
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
    let result =
      tokio::task::spawn_blocking(move || clone_blocking(&options, ssh_command, &cancel)).await?;
    drop(agent);
    result
  }
}

/// grit is synchronous, so stop its blocking work if the awaiting future is dropped.
struct CancelOnDrop(CancelToken);

impl Drop for CancelOnDrop {
  fn drop(&mut self) {
    self.0.cancel();
  }
}

fn clone_blocking(
  options: &CloneOptions,
  ssh_command: Option<String>,
  cancel: &CancelToken,
) -> Result<(), GitError> {
  let dest = options.destination();
  let dest_existed = dest.exists();

  let mut grit_options = grit_clone::CloneOptions::new(options.url(), dest.to_path_buf());
  grit_options.branch = options.branch().map(str::to_owned);
  grit_options.depth = options.depth().map(NonZeroU32::get);
  if let Some(ssh_command) = ssh_command {
    grit_options.environment = grit_options
      .environment
      .with_var("GIT_SSH_COMMAND", ssh_command)
      .with_var("GIT_SSH_VARIANT", "ssh");
  }
  let http_factory = match (options.auth(), RemoteUrl::parse(options.url())?) {
    (Some(Auth::Https { user, pass }), RemoteUrl::Http(url)) => Some(StaticCredentialsFactory {
      credentials: StaticCredentials::new(&url, user, pass),
      environment: grit_options.environment.clone(),
    }),
    _ => None,
  };

  let result = grit_clone::clone(
    grit_options,
    &mut NullProgress,
    cancel,
    http_factory
      .as_ref()
      .map(|factory| factory as &dyn HttpClientFactory),
  );

  match result {
    Ok(_) => Ok(()),
    Err(err) => {
      if grit_clone::should_cleanup_failed_clone(&err) {
        grit_clone::cleanup_failed_clone(dest, dest_existed);
      }
      Err(classify_failure(options, &err).unwrap_or(GitError::Grit(err)))
    }
  }
}

fn classify_failure(options: &CloneOptions, err: &Error) -> Option<GitError> {
  let url = redact(options.url());
  let Error::Remote(err) = err else {
    return None;
  };
  Some(match err {
    RemoteError::Auth { .. } | RemoteError::CredentialsUnavailable { .. } => {
      GitError::AuthenticationFailed { url }
    }
    RemoteError::HttpStatus {
      status: 401 | 403, ..
    } => GitError::AuthenticationFailed { url },
    RemoteError::HttpStatus { status: 404, .. } => GitError::RepositoryNotFound { url },
    RemoteError::Unreadable {
      reason: RemoteUnreadableReason::RepositoryNotFound,
      ..
    } => GitError::RepositoryNotFound { url },
    RemoteError::Unreadable {
      reason: RemoteUnreadableReason::HttpStatus { status: 404, .. },
      ..
    } => GitError::RepositoryNotFound { url },
    RemoteError::Unreadable { .. } | RemoteError::Transport { .. } => GitError::RemoteUnreachable {
      url,
      reason: err.to_string(),
    },
    RemoteError::Clone(clone) => match clone.as_ref() {
      CloneError::DestNotEmpty { .. } => GitError::DestinationNotEmpty {
        path: options.destination().to_path_buf(),
      },
      CloneError::RemoteRefNotFound { name } => GitError::BranchNotFound {
        branch: options.branch().unwrap_or(name).to_owned(),
      },
      _ => return None,
    },
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
    Self {
      protocol: url.scheme().to_owned(),
      host,
      user: user.to_owned(),
      pass: pass.to_owned(),
    }
  }
}

impl CredentialProvider for StaticCredentials {
  fn fill(&self, input: &Credential) -> grit_lib::error::Result<Credential> {
    let mut filled = input.clone();
    // Never hand the credentials to another origin, e.g. after a redirect.
    if input.protocol.as_deref() == Some(&self.protocol)
      && input.host.as_deref() == Some(&self.host)
    {
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

  fn create_for_preflight(
    &self,
    config: &ConfigSet,
  ) -> grit_lib::error::Result<Box<dyn HttpClient>> {
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
    Credential {
      protocol: Some(protocol.into()),
      host: Some(host.into()),
      ..Credential::default()
    }
  }

  #[test]
  fn fills_credentials_for_the_same_origin() {
    let filled = credentials()
      .fill(&request("https", "example.com:8443"))
      .unwrap();
    assert_eq!(filled.username.as_deref(), Some("user"));
    assert_eq!(filled.password.as_deref(), Some("hunter2"));
  }

  #[test]
  fn withholds_credentials_from_other_origins() {
    for (protocol, host) in [
      ("https", "evil.example.com"),
      ("https", "example.com"),
      ("http", "example.com:8443"),
    ] {
      let filled = credentials().fill(&request(protocol, host)).unwrap();
      assert_eq!(filled.password, None, "{protocol}://{host}");
    }
  }

  #[test]
  fn factory_builds_a_client() {
    let factory = StaticCredentialsFactory {
      credentials: credentials(),
      environment: Environment::empty(),
    };
    let config = ConfigSet::default();
    assert!(factory.create(&config).is_ok());
    assert!(factory.create_for_preflight(&config).is_ok());
  }

  #[test]
  fn classifies_grit_failures() {
    let options =
      CloneOptions::new("https://u:secret@example.com/a.git", "/tmp/b").with_branch("nope");
    let remote = |err: RemoteError| Error::Remote(err);
    let cases: Vec<(Error, Expect)> = vec![
      (
        remote(RemoteError::Auth {
          url: "https://example.com/a.git".into(),
          reason: AuthFailure::Rejected,
        }),
        |e| matches!(e, GitError::AuthenticationFailed { .. }),
      ),
      (
        remote(RemoteError::HttpStatus {
          url: None,
          status: 403,
        }),
        |e| matches!(e, GitError::AuthenticationFailed { .. }),
      ),
      (
        remote(RemoteError::Unreadable {
          url: None,
          reason: RemoteUnreadableReason::RepositoryNotFound,
        }),
        |e| matches!(e, GitError::RepositoryNotFound { .. }),
      ),
      (
        remote(RemoteError::Unreadable {
          url: None,
          reason: RemoteUnreadableReason::HttpStatus { status: 404 },
        }),
        |e| matches!(e, GitError::RepositoryNotFound { .. }),
      ),
      (
        remote(RemoteError::Unreadable {
          url: None,
          reason: RemoteUnreadableReason::NoHandshake,
        }),
        |e| matches!(e, GitError::RemoteUnreachable { .. }),
      ),
      (
        remote(RemoteError::Clone(Box::new(
          CloneError::RemoteRefNotFound {
            name: "nope".into(),
          },
        ))),
        |e| matches!(e, GitError::BranchNotFound { branch } if branch == "nope"),
      ),
      (
        remote(RemoteError::Clone(Box::new(CloneError::DestNotEmpty {
          path: "/tmp/b".into(),
        }))),
        |e| matches!(e, GitError::DestinationNotEmpty { .. }),
      ),
    ];
    for (err, expected) in &cases {
      let classified = classify_failure(&options, err).unwrap();
      assert!(expected(&classified), "{err:?} => {classified:?}");
      assert!(!classified.to_string().contains("secret"), "{classified}");
    }
  }

  #[test]
  fn leaves_other_failures_unclassified() {
    let options = CloneOptions::new("https://example.com/a.git", "/tmp/b");
    assert!(classify_failure(&options, &Error::Cancelled).is_none());
  }
}

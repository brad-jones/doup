use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use std::fmt;

/// Credentials used to reach a remote; mirrors the shape of doup's bot config.
#[derive(Clone, PartialEq, Eq)]
pub enum Auth {
  Https {
    /// The username to use with a HTTPS origin.
    user: String,

    /// The password (usually a token) to use with a HTTPS origin.
    pass: String,
  },
  Ssh {
    /// An OpenSSH private key, not protected by a passphrase.
    key: String,
  },
}

impl Auth {
  pub(crate) fn kind(&self) -> &'static str {
    match self {
      Self::Https { .. } => "https",
      Self::Ssh { .. } => "ssh",
    }
  }
}

impl fmt::Debug for Auth {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    const REDACTED: &str = "<redacted>";
    match self {
      Self::Https { user, .. } => f
        .debug_struct("Https")
        .field("user", user)
        .field("pass", &REDACTED)
        .finish(),
      Self::Ssh { .. } => f.debug_struct("Ssh").field("key", &REDACTED).finish(),
    }
  }
}

/// The `Authorization` header value Git sends for HTTPS basic auth.
pub(crate) fn basic_auth_header(user: &str, pass: &str) -> String {
  format!(
    "Authorization: Basic {}",
    STANDARD.encode(format!("{user}:{pass}"))
  )
}

#[cfg(test)]
mod tests {
  use super::*;

  const KEY: &str = "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n-----END OPENSSH PRIVATE KEY-----";

  #[test]
  fn debug_redacts_secrets() {
    let https = Auth::Https {
      user: "me".into(),
      pass: "hunter2".into(),
    };
    let ssh = Auth::Ssh { key: KEY.into() };
    let out = format!("{https:?} {ssh:?}");
    assert!(!out.contains("hunter2"), "{out}");
    assert!(!out.contains("OPENSSH"), "{out}");
    assert!(out.contains("me"), "{out}");
  }

  #[test]
  fn basic_auth_header_is_base64_user_colon_pass() {
    assert_eq!(
      basic_auth_header("user", "pass"),
      "Authorization: Basic dXNlcjpwYXNz"
    );
  }
}

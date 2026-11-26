use crate::GitError;

/// Who commits made by doup are attributed to, as both author and committer.
///
/// Defaults to `doup[bot] <doup[bot]@users.noreply.github.com>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Identity {
  name: String,
  email: String,
}

impl Identity {
  /// The name and email address to record on commits.
  ///
  /// # Errors
  ///
  /// Returns [`GitError::InvalidIdentity`] when either is blank or holds `<`, `>` or a control
  /// character. git would silently strip those, but grit refuses them, so both are refused up
  /// front to keep the backends identical.
  pub fn new(name: impl Into<String>, email: impl Into<String>) -> Result<Self, GitError> {
    let (name, email) = (name.into(), email.into());
    let invalid =
      |value: &str| value.trim().is_empty() || value.chars().any(|c| matches!(c, '<' | '>') || c.is_control());
    if invalid(&name) || invalid(&email) {
      return Err(GitError::InvalidIdentity { name, email });
    }
    Ok(Self { name, email })
  }

  pub fn name(&self) -> &str {
    &self.name
  }

  pub fn email(&self) -> &str {
    &self.email
  }

  /// The environment variables that make both git and grit record this identity.
  pub(crate) fn envs(&self) -> [(&'static str, &str); 4] {
    [
      ("GIT_AUTHOR_NAME", &self.name),
      ("GIT_AUTHOR_EMAIL", &self.email),
      ("GIT_COMMITTER_NAME", &self.name),
      ("GIT_COMMITTER_EMAIL", &self.email),
    ]
  }
}

impl Default for Identity {
  fn default() -> Self {
    Self { name: "doup[bot]".into(), email: "doup[bot]@users.noreply.github.com".into() }
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn rejects_what_cannot_go_in_an_ident_line() {
    for (name, email) in [("", "a@x"), ("a", " "), ("a <b>", "a@x"), ("a", "a@x>"), ("a\nb", "a@x"), ("a", "a@x\t")] {
      let err = Identity::new(name, email).unwrap_err();
      assert!(matches!(err, GitError::InvalidIdentity { .. }), "{name:?} {email:?}");
    }
    assert_eq!(Identity::new("Jane Doe", "jane@example.com").unwrap().name(), "Jane Doe");
  }
}

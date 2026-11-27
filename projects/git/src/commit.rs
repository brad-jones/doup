use crate::GitError;

/// A commit read from a clone, as returned by [`crate::ClonedRepo::branch_commits`].
///
/// It is parsed from the raw commit object, so [`Self::is_merge`] reflects the parents the commit
/// really has, even when it sits on a shallow boundary and its parents were never fetched.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GitCommit {
  id: String,
  author_name: String,
  author_email: String,
  message: String,
  parents: Vec<String>,
}

impl GitCommit {
  /// Parses the raw bytes of commit object `id`: its headers, a blank line, then its message.
  ///
  /// Non UTF-8 text (e.g. a commit with an `encoding` header) is converted lossily.
  pub(crate) fn parse(id: String, raw: &[u8]) -> Result<Self, GitError> {
    let (headers, message) = match raw.windows(2).position(|pair| pair == b"\n\n") {
      Some(end) => (&raw[..end], &raw[end + 2..]),
      None => (raw, &[][..]),
    };

    let mut parents = Vec::new();
    let mut author = None;
    // Continuation lines of multi-line headers (e.g. `gpgsig`) start with a space, so never match.
    for line in headers.split(|&byte| byte == b'\n') {
      if let Some(parent) = line.strip_prefix(b"parent ") {
        parents.push(String::from_utf8_lossy(parent).trim().to_owned());
      } else if let Some(ident) = line.strip_prefix(b"author ") {
        author.get_or_insert(ident);
      }
    }

    let Some((author_name, author_email)) = author.and_then(split_ident) else {
      return Err(GitError::InvalidCommit { id });
    };
    Ok(Self { id, author_name, author_email, message: String::from_utf8_lossy(message).into_owned(), parents })
  }

  /// The commit's full hex object id.
  pub fn id(&self) -> &str {
    &self.id
  }

  /// The author's name, like `git show -s --format=%an`.
  pub fn author_name(&self) -> &str {
    &self.author_name
  }

  /// The author's email address, like `git show -s --format=%ae`.
  pub fn author_email(&self) -> &str {
    &self.author_email
  }

  /// The full commit message (subject and body) exactly as stored, like `git show -s --format=%B`.
  pub fn message(&self) -> &str {
    &self.message
  }

  /// The full hex ids of the commit's parents, in order, like `git show -s --format=%P`.
  ///
  /// Read from the commit object itself, so they are listed even when they were never fetched.
  pub fn parents(&self) -> &[String] {
    &self.parents
  }

  /// Whether the commit has more than one parent.
  pub fn is_merge(&self) -> bool {
    self.parents.len() > 1
  }
}

/// Splits an ident, `Name <email> <timestamp> <tz>`, into its name and email the way git does: the
/// email runs from the first `<` to the next `>`.
fn split_ident(ident: &[u8]) -> Option<(String, String)> {
  let open = ident.iter().position(|&byte| byte == b'<')?;
  let close = open + ident[open..].iter().position(|&byte| byte == b'>')?;
  let name = String::from_utf8_lossy(&ident[..open]).trim().to_owned();
  let email = String::from_utf8_lossy(&ident[open + 1..close]).into_owned();
  Some((name, email))
}

#[cfg(test)]
mod tests {
  use super::*;

  fn parse(raw: &str) -> Result<GitCommit, GitError> {
    GitCommit::parse("abc".into(), raw.as_bytes())
  }

  #[test]
  fn parses_a_commit() {
    let commit = parse(
      "tree t\nparent p\nauthor Jane Doe <jane@example.com> 1 +0000\ncommitter C <c@x> 1 +0000\n\nsubject\n\nbody\n",
    )
    .unwrap();
    assert_eq!(commit.id(), "abc");
    assert_eq!(commit.author_name(), "Jane Doe");
    assert_eq!(commit.author_email(), "jane@example.com");
    assert_eq!(commit.message(), "subject\n\nbody\n");
    assert_eq!(commit.parents(), ["p"]);
    assert!(!commit.is_merge());
  }

  #[test]
  fn counts_every_parent_and_skips_multi_line_headers() {
    let commit = parse(
      "tree t\nparent a\nparent b\nauthor A <a@x> 1 +0000\ncommitter C <c@x> 1 +0000\ngpgsig -----BEGIN-----\n parent c\n author B <b@x>\n -----END-----\n\nmerge\n",
    )
    .unwrap();
    assert!(commit.is_merge());
    assert_eq!(commit.parents(), ["a", "b"]);
    assert_eq!(commit.author_email(), "a@x");
  }

  #[test]
  fn a_root_commit_without_a_message_is_not_a_merge() {
    let commit = parse("tree t\nauthor <> 1 +0000\ncommitter C <c@x> 1 +0000").unwrap();
    assert!(!commit.is_merge());
    assert!(commit.parents().is_empty());
    assert_eq!((commit.author_name(), commit.author_email(), commit.message()), ("", "", ""));
  }

  #[test]
  fn rejects_a_commit_without_a_valid_author() {
    for raw in ["tree t\ncommitter C <c@x> 1 +0000\n\nm\n", "tree t\nauthor nobody 1 +0000\n\nm\n"] {
      assert!(matches!(parse(raw), Err(GitError::InvalidCommit { id }) if id == "abc"), "{raw}");
    }
  }
}

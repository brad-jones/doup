use crate::GitError;
use url::Url;

/// A remote URL classified by the transport Git would use to reach it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RemoteUrl {
  Http(Url),
  Ssh,
  Other,
}

impl RemoteUrl {
  /// Classifies `url` the same way Git does: `scheme://`, then scp-like `host:path`, then a path.
  pub(crate) fn parse(url: &str) -> Result<Self, GitError> {
    let invalid = || GitError::InvalidUrl { url: redact(url) };
    if url.trim().is_empty() {
      return Err(invalid());
    }
    if let Some((scheme, _)) = url.split_once("://") {
      return match scheme.to_ascii_lowercase().as_str() {
        "http" | "https" => {
          let parsed = Url::parse(url).map_err(|_| invalid())?;
          parsed.host_str().ok_or_else(invalid)?;
          Ok(Self::Http(parsed))
        }
        "ssh" | "git+ssh" | "ssh+git" => Ok(Self::Ssh),
        "git" | "file" => Ok(Self::Other),
        _ => Err(invalid()),
      };
    }
    if is_scp_like(url) {
      return Ok(Self::Ssh);
    }
    Ok(Self::Other)
  }

  /// The config key Git reads an extra HTTP header from for this remote's origin.
  pub(crate) fn extra_header_key(&self) -> Option<String> {
    match self {
      Self::Http(url) => Some(format!(
        "http.{}/.extraheader",
        url.origin().ascii_serialization()
      )),
      _ => None,
    }
  }
}

/// Git treats `host:path` as ssh when the colon comes before any slash (and isn't a drive letter).
fn is_scp_like(url: &str) -> bool {
  let Some(colon) = url.find(':') else {
    return false;
  };
  let is_drive_letter = colon == 1 && url.as_bytes()[0].is_ascii_alphabetic();
  !is_drive_letter && !url[..colon].contains('/')
}

/// Strips any password from `url` so it is safe to display.
pub(crate) fn redact(url: &str) -> String {
  match Url::parse(url) {
    Ok(mut parsed) if parsed.password().is_some() => {
      let _ = parsed.set_password(None);
      parsed.to_string()
    }
    _ => url.to_owned(),
  }
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn classifies_http_urls() {
    for url in [
      "https://github.com/a/b.git",
      "http://example.com:8080/a/b",
      "HTTPS://github.com/a/b",
    ] {
      assert!(
        matches!(RemoteUrl::parse(url), Ok(RemoteUrl::Http(_))),
        "{url}"
      );
    }
  }

  #[test]
  fn classifies_ssh_urls() {
    for url in [
      "ssh://git@github.com/a/b.git",
      "git+ssh://git@github.com/a/b.git",
      "git@github.com:a/b.git",
      "github.com:a/b.git",
    ] {
      assert_eq!(RemoteUrl::parse(url).unwrap(), RemoteUrl::Ssh, "{url}");
    }
  }

  #[test]
  fn classifies_other_urls() {
    for url in [
      "file:///srv/repo.git",
      "git://example.com/repo.git",
      "/srv/repo.git",
      "./repo",
      "C:\\repos\\repo",
      "C:/repos/repo",
    ] {
      assert_eq!(RemoteUrl::parse(url).unwrap(), RemoteUrl::Other, "{url}");
    }
  }

  #[test]
  fn rejects_invalid_urls() {
    for url in ["", "   ", "ftp://example.com/repo", "https://"] {
      assert!(
        matches!(RemoteUrl::parse(url), Err(GitError::InvalidUrl { .. })),
        "{url}"
      );
    }
  }

  #[test]
  fn extra_header_key_uses_the_origin() {
    let key = |url: &str| RemoteUrl::parse(url).unwrap().extra_header_key();
    assert_eq!(
      key("https://github.com/a/b.git").as_deref(),
      Some("http.https://github.com/.extraheader")
    );
    assert_eq!(
      key("https://user@example.com:8443/a/b").as_deref(),
      Some("http.https://example.com:8443/.extraheader")
    );
    assert_eq!(
      key("https://example.com:443/a").as_deref(),
      Some("http.https://example.com/.extraheader")
    );
    assert_eq!(key("git@github.com:a/b.git"), None);
  }

  #[test]
  fn redact_removes_passwords_only() {
    assert_eq!(
      redact("https://user:secret@example.com/a.git"),
      "https://user@example.com/a.git"
    );
    assert_eq!(
      redact("https://example.com/a.git"),
      "https://example.com/a.git"
    );
    assert_eq!(redact("git@github.com:a/b.git"), "git@github.com:a/b.git");
  }

  #[test]
  fn invalid_url_error_is_redacted() {
    let err = RemoteUrl::parse("ftp://user:secret@example.com/a").unwrap_err();
    assert!(!err.to_string().contains("secret"), "{err}");
  }
}

//! Builds the REST requests for creating, updating & closing pull requests and parses their responses.
//!
//! Like [`crate::graphql`], this module only uses plain `std` / `serde` types (plus the WIT records)
//! so that it can be unit tested natively, without a wasm runtime.

use crate::{PullRequest, PullRequestContent, version};
use serde::Deserialize;
use serde_json::{Value, json};

/// GitHub rejects a pull request body longer than this many characters.
const MAX_BODY_CHARS: usize = 65_536;

/// Appended to a body that had to be truncated to fit within [`MAX_BODY_CHARS`].
const TRUNCATED_NOTICE: &str = "\n\n---\n\n_This description was truncated to fit within GitHub's size limit._";

/// The JSON body that opens a pull request from `head_branch`.
/// See <https://docs.github.com/en/rest/pulls/pulls#create-a-pull-request>.
pub fn create_pr_request(head_branch: &str, content: &PullRequestContent) -> Result<Value, String> {
  if head_branch.trim().is_empty() {
    return Err("head branch must not be empty".to_string());
  }
  let mut request = update_pr_request(content)?;
  request["head"] = json!(head_branch);
  Ok(request)
}

/// The JSON body that replaces the base branch, title & body of a pull request.
/// See <https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request>.
pub fn update_pr_request(content: &PullRequestContent) -> Result<Value, String> {
  if content.base_branch.trim().is_empty() {
    return Err("base branch must not be empty".to_string());
  }
  if content.title.trim().is_empty() {
    return Err("title must not be empty".to_string());
  }
  Ok(json!({ "title": content.title, "body": pr_body(content)?, "base": content.base_branch }))
}

/// The JSON body that closes a pull request.
/// See <https://docs.github.com/en/rest/pulls/pulls#update-a-pull-request>.
pub fn close_pr_request() -> Value {
  json!({ "state": "closed" })
}

/// The JSON body that comments on a pull request with `body`, truncated to fit GitHub's limit.
/// Returns `None` when `body` is blank, as there is nothing worth posting.
/// See <https://docs.github.com/en/rest/issues/comments#create-an-issue-comment>.
pub fn comment_request(body: &str) -> Option<Value> {
  if body.trim().is_empty() {
    return None;
  }
  Some(json!({ "body": truncate(body.to_string()) }))
}

/// The pull request body: the version marker followed by `content.body`, truncated to fit GitHub's limit.
///
/// The marker comes first so that it is never truncated, and so that [`version::parse`], which takes
/// the first marker it finds, is never fooled by marker-like text in the body (eg: quoted release notes).
fn pr_body(content: &PullRequestContent) -> Result<String, String> {
  let version = &content.version;
  if version.is_empty() || version.chars().any(char::is_whitespace) || version.contains("-->") {
    return Err(format!("invalid version {version:?}, it must be non-empty without whitespace or `-->`"));
  }

  let mut body = version::marker(version);
  if !content.body.trim().is_empty() {
    body.push_str("\n\n");
    body.push_str(&content.body);
  }
  Ok(truncate(body))
}

/// Truncates `body` (on a char boundary) to at most [`MAX_BODY_CHARS`], appending [`TRUNCATED_NOTICE`] when it was.
fn truncate(mut body: String) -> String {
  if body.chars().count() <= MAX_BODY_CHARS {
    return body;
  }
  let keep = MAX_BODY_CHARS - TRUNCATED_NOTICE.chars().count();
  let end = body.char_indices().nth(keep).map_or(body.len(), |(index, _)| index);
  body.truncate(end);
  body.push_str(TRUNCATED_NOTICE);
  body
}

#[derive(Deserialize)]
struct PrResponse {
  number: i64,
  html_url: String,
  head: BranchRef,
  base: BranchRef,
  body: Option<String>,
}

#[derive(Deserialize)]
struct BranchRef {
  #[serde(rename = "ref")]
  name: String,
}

/// Parses a pull request, as returned by the create & update endpoints.
pub fn parse_pr(response: Value) -> Result<PullRequest, String> {
  let pr: PrResponse =
    serde_json::from_value(response).map_err(|err| format!("invalid GitHub pull request response: {err}"))?;
  Ok(PullRequest {
    number: pr.number,
    url: pr.html_url,
    head_branch: pr.head.name,
    base_branch: pr.base.name,
    version: version::parse(pr.body.as_deref()),
  })
}

#[derive(Deserialize)]
struct ErrorResponse {
  message: String,
  #[serde(default)]
  errors: Vec<ErrorDetail>,
}

#[derive(Deserialize)]
struct ErrorDetail {
  message: Option<String>,
  resource: Option<String>,
  field: Option<String>,
  code: Option<String>,
}

impl ErrorDetail {
  fn describe(self) -> Option<String> {
    if let Some(message) = self.message {
      return Some(message);
    }
    let subject = [self.resource, self.field].into_iter().flatten().collect::<Vec<_>>().join(".");
    match self.code {
      Some(code) if subject.is_empty() => Some(code),
      Some(code) => Some(format!("{subject} {code}")),
      None if subject.is_empty() => None,
      None => Some(format!("{subject} invalid")),
    }
  }
}

/// A human readable description of a REST error response `body`.
///
/// eg: `Validation Failed: A pull request already exists for acme:doup/cargo/serde.`
pub fn error_message(body: &str) -> String {
  let Ok(response) = serde_json::from_str::<ErrorResponse>(body) else {
    let body = body.trim();
    return match body.char_indices().nth(200) {
      Some((end, _)) => format!("{}...", &body[..end]),
      None if body.is_empty() => "no error message".to_string(),
      None => body.to_string(),
    };
  };
  let details: Vec<_> = response.errors.into_iter().filter_map(ErrorDetail::describe).collect();
  if details.is_empty() { response.message } else { format!("{}: {}", response.message, details.join("; ")) }
}

#[cfg(test)]
mod tests {
  use super::*;

  fn content(body: &str, version: &str) -> PullRequestContent {
    PullRequestContent {
      base_branch: "main".to_string(),
      title: "chore(doup): update serde".to_string(),
      body: body.to_string(),
      version: version.to_string(),
    }
  }

  #[test]
  fn the_create_request_includes_the_head_branch() {
    let request = create_pr_request("doup/cargo/serde", &content("Bumps serde.", "1.2.3")).unwrap();
    assert_eq!(
      request,
      json!({
        "title": "chore(doup): update serde",
        "body": "<!-- doup:version=1.2.3 -->\n\nBumps serde.",
        "base": "main",
        "head": "doup/cargo/serde",
      })
    );
  }

  #[test]
  fn the_update_request_never_includes_a_head_branch() {
    let request = update_pr_request(&content("", "1.2.3")).unwrap();
    assert_eq!(
      request,
      json!({ "title": "chore(doup): update serde", "body": "<!-- doup:version=1.2.3 -->", "base": "main" })
    );
  }

  #[test]
  fn the_version_round_trips_even_when_the_body_contains_a_marker() {
    let request = update_pr_request(&content("<!-- doup:version=9.9.9 -->", "1.2.3")).unwrap();
    assert_eq!(version::parse(request["body"].as_str()), "1.2.3");
  }

  #[test]
  fn invalid_content_is_rejected() {
    for version in ["", "1 2", "1-->2"] {
      assert!(update_pr_request(&content("", version)).is_err(), "{version:?}");
    }
    let mut no_title = content("", "1.2.3");
    no_title.title = " ".to_string();
    assert!(update_pr_request(&no_title).is_err());
    let mut no_base = content("", "1.2.3");
    no_base.base_branch = String::new();
    assert!(update_pr_request(&no_base).is_err());
    assert!(create_pr_request(" ", &content("", "1.2.3")).is_err());
  }

  #[test]
  fn a_long_body_is_truncated_on_a_char_boundary() {
    let request = update_pr_request(&content(&"é".repeat(MAX_BODY_CHARS), "1.2.3")).unwrap();
    let body = request["body"].as_str().unwrap();
    assert_eq!(body.chars().count(), MAX_BODY_CHARS);
    assert!(body.ends_with(TRUNCATED_NOTICE));
    assert_eq!(version::parse(Some(body)), "1.2.3");
  }

  #[test]
  fn a_body_at_the_limit_is_left_alone() {
    let marker_len = version::marker("1.2.3").chars().count() + 2;
    let request = update_pr_request(&content(&"a".repeat(MAX_BODY_CHARS - marker_len), "1.2.3")).unwrap();
    let body = request["body"].as_str().unwrap();
    assert_eq!(body.chars().count(), MAX_BODY_CHARS);
    assert!(!body.ends_with(TRUNCATED_NOTICE));
  }

  #[test]
  fn a_pull_request_response_is_parsed() {
    let pr = parse_pr(json!({
      "number": 42,
      "html_url": "https://github.com/acme/foo/pull/42",
      "head": { "ref": "doup/cargo/serde", "sha": "abc" },
      "base": { "ref": "main", "sha": "def" },
      "body": "<!-- doup:version=1.2.3 -->\n\nBumps serde.",
      "state": "open",
    }))
    .unwrap();
    assert_eq!(pr.number, 42);
    assert_eq!(pr.url, "https://github.com/acme/foo/pull/42");
    assert_eq!(pr.head_branch, "doup/cargo/serde");
    assert_eq!(pr.base_branch, "main");
    assert_eq!(pr.version, "1.2.3");
  }

  #[test]
  fn a_garbage_response_is_an_error() {
    assert!(parse_pr(json!({ "number": 1 })).is_err());
  }

  #[test]
  fn blank_comments_are_not_posted() {
    for body in ["", " ", "\n\t"] {
      assert_eq!(comment_request(body), None, "{body:?}");
    }
    assert_eq!(comment_request("Superseded."), Some(json!({ "body": "Superseded." })));
  }

  #[test]
  fn a_long_comment_is_truncated() {
    let request = comment_request(&"é".repeat(MAX_BODY_CHARS + 1)).unwrap();
    let body = request["body"].as_str().unwrap();
    assert_eq!(body.chars().count(), MAX_BODY_CHARS);
    assert!(body.ends_with(TRUNCATED_NOTICE));
  }

  #[test]
  fn error_messages_include_the_details() {
    let body = r#"{"message":"Validation Failed","errors":[
      {"resource":"PullRequest","code":"custom","message":"A pull request already exists for acme:doup/x."},
      {"resource":"PullRequest","field":"base","code":"invalid"},
      {"code":"missing"},
      {}
    ]}"#;
    assert_eq!(
      error_message(body),
      "Validation Failed: A pull request already exists for acme:doup/x.; PullRequest.base invalid; missing"
    );
    assert_eq!(error_message(r#"{"message":"Not Found"}"#), "Not Found");
  }

  #[test]
  fn unparseable_error_bodies_are_shortened() {
    assert_eq!(error_message(""), "no error message");
    assert_eq!(error_message(" <html> "), "<html>");
    assert_eq!(error_message(&"x".repeat(300)), format!("{}...", "x".repeat(200)));
  }
}

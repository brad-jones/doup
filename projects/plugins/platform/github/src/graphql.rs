//! Builds the GraphQL queries for listing pull requests and parses their responses.
//!
//! This module only uses plain `std` / `serde` types (plus the WIT records) so that it can be
//! unit tested natively, without a wasm runtime.

use crate::{PullRequest, version};
use serde::Deserialize;
use serde_json::{Map, Value, json};
use std::collections::BTreeMap;

/// GitHub caps `first` at 100.
const PAGE_SIZE: usize = 100;

/// How many branches are looked up per closed PR query, keeping each query well within GitHub's limits.
pub const BRANCHES_PER_QUERY: usize = 25;

const PR_FIELDS: &str = "number url headRefName baseRefName body isCrossRepository";

/// A query for one page of the open pull requests in `owner/name`, newest first.
pub fn open_prs_query(owner: &str, name: &str, cursor: Option<&str>) -> Value {
  let query = format!(
    "query($owner: String!, $name: String!, $cursor: String) {{
  repository(owner: $owner, name: $name) {{
    pullRequests(states: OPEN, first: {PAGE_SIZE}, after: $cursor, orderBy: {{field: CREATED_AT, direction: DESC}}) {{
      pageInfo {{ hasNextPage endCursor }}
      nodes {{ {PR_FIELDS} }}
    }}
  }}
}}"
  );
  json!({ "query": query, "variables": { "owner": owner, "name": name, "cursor": cursor } })
}

/// A query for the closed (not merged) pull requests in `owner/name` whose head branch is one of `branches`.
///
/// Each branch gets its own aliased `pullRequests` field (`b0`, `b1`, ...), with the branch passed as a
/// variable so that it is never interpolated into the query. Only the newest 100 PRs per branch are returned.
pub fn closed_prs_query(owner: &str, name: &str, branches: &[String]) -> Value {
  let params: String = (0..branches.len()).map(|i| format!(", $b{i}: String!")).collect();
  let fields: String = (0..branches.len())
    .map(|i| {
      format!(
        "\n    b{i}: pullRequests(headRefName: $b{i}, states: CLOSED, first: {PAGE_SIZE}, orderBy: {{field: CREATED_AT, direction: DESC}}) {{ nodes {{ {PR_FIELDS} }} }}"
      )
    })
    .collect();
  let query = format!(
    "query($owner: String!, $name: String!{params}) {{\n  repository(owner: $owner, name: $name) {{{fields}\n  }}\n}}"
  );

  let mut variables = Map::new();
  variables.insert("owner".to_string(), json!(owner));
  variables.insert("name".to_string(), json!(name));
  for (i, branch) in branches.iter().enumerate() {
    variables.insert(format!("b{i}"), json!(branch));
  }
  json!({ "query": query, "variables": variables })
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrNode {
  number: i64,
  url: String,
  head_ref_name: String,
  base_ref_name: String,
  body: Option<String>,
  is_cross_repository: bool,
}

impl PrNode {
  fn into_pull_request(self) -> PullRequest {
    PullRequest {
      number: self.number,
      url: self.url,
      head_branch: self.head_ref_name,
      base_branch: self.base_ref_name,
      version: version::parse(self.body.as_deref()),
    }
  }
}

#[derive(Deserialize)]
struct Connection {
  #[serde(rename = "pageInfo")]
  page_info: Option<PageInfo>,
  nodes: Vec<PrNode>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PageInfo {
  has_next_page: bool,
  end_cursor: Option<String>,
}

#[derive(Deserialize)]
struct Data<T> {
  repository: Option<T>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct OpenRepository {
  pull_requests: Connection,
}

/// One page of open pull requests.
pub struct OpenPrsPage {
  pub pull_requests: Vec<PullRequest>,
  /// The cursor for the next page, if there is one.
  pub next_cursor: Option<String>,
}

/// Parses the `data` of an [`open_prs_query`], keeping PRs whose head branch starts with `head_branch_prefix`.
pub fn parse_open_prs(data: Value, head_branch_prefix: &str) -> Result<OpenPrsPage, String> {
  let data: Data<OpenRepository> =
    serde_json::from_value(data).map_err(|err| format!("invalid GitHub open pull requests response: {err}"))?;
  let connection = data.repository.ok_or_else(repository_not_found)?.pull_requests;

  let next_cursor = connection.page_info.filter(|p| p.has_next_page).and_then(|p| p.end_cursor);
  let pull_requests = connection
    .nodes
    .into_iter()
    .filter(|pr| !pr.is_cross_repository && pr.head_ref_name.starts_with(head_branch_prefix))
    .map(PrNode::into_pull_request)
    .collect();
  Ok(OpenPrsPage { pull_requests, next_cursor })
}

/// Parses the `data` of a [`closed_prs_query`].
pub fn parse_closed_prs(data: Value) -> Result<Vec<PullRequest>, String> {
  let data: Data<BTreeMap<String, Connection>> =
    serde_json::from_value(data).map_err(|err| format!("invalid GitHub closed pull requests response: {err}"))?;
  let connections = data.repository.ok_or_else(repository_not_found)?;

  // Order by alias index so results follow the order the branches were asked for.
  let mut connections: Vec<_> = connections.into_iter().collect();
  connections.sort_by_key(|(alias, _)| alias.trim_start_matches('b').parse::<usize>().unwrap_or(usize::MAX));

  Ok(
    connections
      .into_iter()
      .flat_map(|(_, connection)| connection.nodes)
      .filter(|pr| !pr.is_cross_repository)
      .map(PrNode::into_pull_request)
      .collect(),
  )
}

fn repository_not_found() -> String {
  "repository not found, or the github_token can not access it".to_string()
}

#[cfg(test)]
mod tests {
  use super::*;

  fn node(number: i64, branch: &str, body: Option<&str>, fork: bool) -> Value {
    json!({
      "number": number,
      "url": format!("https://github.com/acme/foo/pull/{number}"),
      "headRefName": branch,
      "baseRefName": "main",
      "body": body,
      "isCrossRepository": fork,
    })
  }

  fn summary(prs: &[PullRequest]) -> Vec<(i64, &str, &str)> {
    prs.iter().map(|pr| (pr.number, pr.head_branch.as_str(), pr.version.as_str())).collect()
  }

  #[test]
  fn the_open_query_passes_everything_as_variables() {
    let query = open_prs_query("acme", "foo", Some("abc"));
    assert_eq!(query["variables"], json!({ "owner": "acme", "name": "foo", "cursor": "abc" }));
    assert!(query["query"].as_str().unwrap().contains("states: OPEN"));
    assert_eq!(open_prs_query("acme", "foo", None)["variables"]["cursor"], Value::Null);
  }

  #[test]
  fn the_closed_query_has_an_alias_per_branch() {
    let branches = vec!["doup/a\"b".to_string(), "doup/c".to_string()];
    let query = closed_prs_query("acme", "foo", &branches);
    let text = query["query"].as_str().unwrap();

    assert!(text.contains("$b0: String!, $b1: String!"), "{text}");
    assert!(text.contains("b0: pullRequests(headRefName: $b0, states: CLOSED"), "{text}");
    assert!(text.contains("b1: pullRequests(headRefName: $b1, states: CLOSED"), "{text}");
    assert!(!text.contains("doup/"), "branches must not be interpolated: {text}");
    assert_eq!(query["variables"]["b0"], "doup/a\"b");
    assert_eq!(query["variables"]["b1"], "doup/c");
  }

  #[test]
  fn open_prs_are_filtered_by_prefix_and_forks() {
    let data = json!({ "repository": { "pullRequests": {
      "pageInfo": { "hasNextPage": true, "endCursor": "next" },
      "nodes": [
        node(1, "doup/cargo/serde", Some("<!-- doup:version=1.2.3 -->"), false),
        node(2, "feature/thing", None, false),
        node(3, "doup/cargo/evil", None, true),
        node(4, "doup/cargo/tokio", None, false),
      ],
    }}});
    let page = parse_open_prs(data, "doup/").unwrap();
    assert_eq!(summary(&page.pull_requests), [(1, "doup/cargo/serde", "1.2.3"), (4, "doup/cargo/tokio", "")]);
    assert_eq!(page.pull_requests[0].url, "https://github.com/acme/foo/pull/1");
    assert_eq!(page.pull_requests[0].base_branch, "main");
    assert_eq!(page.next_cursor.as_deref(), Some("next"));
  }

  #[test]
  fn the_last_open_page_has_no_cursor() {
    let data = json!({ "repository": { "pullRequests": {
      "pageInfo": { "hasNextPage": false, "endCursor": "ignored" },
      "nodes": [],
    }}});
    assert_eq!(parse_open_prs(data, "doup/").unwrap().next_cursor, None);
  }

  #[test]
  fn closed_prs_are_collected_in_branch_order() {
    let mut connections = Map::new();
    for i in 0..11 {
      let nodes =
        if i == 1 { vec![node(9, "doup/fork", None, true)] } else { vec![node(i, &format!("doup/{i}"), None, false)] };
      connections.insert(format!("b{i}"), json!({ "nodes": nodes }));
    }
    let prs = parse_closed_prs(json!({ "repository": connections })).unwrap();
    let numbers: Vec<_> = prs.iter().map(|pr| pr.number).collect();
    assert_eq!(numbers, [0, 2, 3, 4, 5, 6, 7, 8, 9, 10]);
  }

  #[test]
  fn a_missing_repository_is_an_error() {
    let err = parse_open_prs(json!({ "repository": null }), "doup/").err().unwrap();
    assert!(err.contains("repository not found"), "{err}");
    assert!(parse_closed_prs(json!({ "repository": null })).is_err());
    assert!(parse_closed_prs(json!("nope")).is_err());
  }
}

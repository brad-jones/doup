// Generated code, the canonical ABI flattens `create-pr(head-branch, content)` into more than 7 core wasm params.
#[allow(clippy::too_many_arguments)]
mod bindings {
  wit_bindgen::generate!({
    world: "platform",
    path: "../../wit",
    default_bindings_module: "crate::bindings",
  });
}

use bindings::*;

mod config;
mod graphql;
mod http;
mod rest;
mod version;

struct DoupPlatformPluginGithub;

impl Guest for DoupPlatformPluginGithub {
  fn configure(config: String) -> Result<(), String> {
    config::init(&config)
  }

  async fn create_pr(head_branch: String, content: PullRequestContent) -> Result<PullRequest, String> {
    let (owner, name) = config::get()?.owner_and_name();
    let request = rest::create_pr_request(&head_branch, &content)?;
    let response = http::rest(http::Method::Post, &format!("/repos/{owner}/{name}/pulls"), &request).await?;
    rest::parse_pr(response)
  }

  async fn update_pr(number: i64, content: PullRequestContent) -> Result<PullRequest, String> {
    if number <= 0 {
      return Err(format!("invalid pull request number {number}"));
    }
    let (owner, name) = config::get()?.owner_and_name();
    let request = rest::update_pr_request(&content)?;
    let response = http::rest(http::Method::Patch, &format!("/repos/{owner}/{name}/pulls/{number}"), &request).await?;
    rest::parse_pr(response)
  }

  async fn list_open_prs(head_branch_prefix: String) -> Result<Vec<PullRequest>, String> {
    let (owner, name) = config::get()?.owner_and_name();
    let mut pull_requests = Vec::new();
    let mut cursor = None;
    loop {
      let data = http::graphql(&graphql::open_prs_query(owner, name, cursor.as_deref())).await?;
      let page = graphql::parse_open_prs(data, &head_branch_prefix)?;
      pull_requests.extend(page.pull_requests);
      match page.next_cursor {
        Some(next) => cursor = Some(next),
        None => return Ok(pull_requests),
      }
    }
  }

  async fn list_closed_prs(mut head_branches: Vec<String>) -> Result<Vec<PullRequest>, String> {
    let (owner, name) = config::get()?.owner_and_name();
    head_branches.sort_unstable();
    head_branches.dedup();

    let mut pull_requests = Vec::new();
    for branches in head_branches.chunks(graphql::BRANCHES_PER_QUERY) {
      let data = http::graphql(&graphql::closed_prs_query(owner, name, branches)).await?;
      pull_requests.extend(graphql::parse_closed_prs(data)?);
    }
    Ok(pull_requests)
  }
}

export!(DoupPlatformPluginGithub);

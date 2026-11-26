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
mod version;

struct DoupPlatformPluginGithub;

impl Guest for DoupPlatformPluginGithub {
  fn configure(config: String) -> Result<(), String> {
    config::init(&config)
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

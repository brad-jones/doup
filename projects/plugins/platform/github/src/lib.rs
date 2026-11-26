wit_bindgen::generate!({
  world: "platform",
  path: "../../wit",
});

use std::sync::OnceLock;

static CONFIG: OnceLock<serde_json::Value> = OnceLock::new();

struct DoupPlatformPluginGithub;

impl Guest for DoupPlatformPluginGithub {
  fn configure(config: String) -> Result<(), String> {
    let value = serde_json::from_str(&config).map_err(|err| format!("invalid config: {err}"))?;
    CONFIG.set(value).map_err(|_| "already configured".to_string())
  }

  async fn list_open_prs(_head_branch_prefix: String) -> Result<Vec<PullRequest>, String> {
    todo!()
  }
  async fn list_closed_prs(_head_branches: Vec<String>) -> Result<Vec<PullRequest>, String> {
    todo!()
  }
}

export!(DoupPlatformPluginGithub);

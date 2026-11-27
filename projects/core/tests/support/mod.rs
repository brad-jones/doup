//! Test support: a real git remote on disk & fake plugins that record what doup did.

#![allow(dead_code)]

use doup_config::bot::{Concurrency, DoupBotRepo};
use doup_config::repo::DoupRepoPlugin;
use doup_core::Updater;
use doup_core::plugins::{Ecosystem, Platform, PluginLoader};
use doup_git::Git;
use doup_plugin_host::PluginError;
use doup_plugin_host::ecosystem_plugin::{Dependency, ReleaseNote, Versions};
use doup_plugin_host::platform_plugin::{PullRequest, PullRequestContent};
use std::collections::{HashMap, HashSet};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tempfile::TempDir;

pub const PLATFORM: &str = "./platform.wasm";
pub const ECOSYSTEM: &str = "./eco.wasm";

/// A bare repo served over `file://`, whose `main` holds a `doup.yaml` naming a platform & an ecosystem plugin.
pub struct Remote {
  dir: TempDir,
  pub url: String,
}

impl Remote {
  pub fn new() -> Self {
    Self::with_plugins(PLATFORM, ECOSYSTEM)
  }

  pub fn with_plugins(platform: &str, ecosystem: &str) -> Self {
    let dir = TempDir::new().unwrap();
    std::fs::write(dir.path().join("gitconfig"), "").unwrap();
    let remote = Self { url: format!("file://{}", dir.path().join("origin.git").display()), dir };

    remote.git(&["init", "-q", "--bare", "-b", "main", "origin.git"]);
    remote.git(&["--git-dir=origin.git", "config", "uploadpack.allowFilter", "true"]);
    remote.git(&["init", "-q", "-b", "main", "src"]);
    remote.git(&["-C", "src", "remote", "add", "origin", "../origin.git"]);
    let config = format!("platform:\n  use: {platform}\necosystems:\n  - use: {ecosystem}\n");
    remote.commit("main", "dev@example.com", "doup.yaml", &config);
    remote
  }

  fn path(&self, name: &str) -> PathBuf {
    self.dir.path().join(name)
  }

  fn git(&self, args: &[&str]) -> String {
    self.git_as("dev@example.com", args)
  }

  /// Runs git in the fixture's dir as `email`, isolated from the developer's own config (eg: commit signing).
  fn git_as(&self, email: &str, args: &[&str]) -> String {
    let output = Command::new("git")
      .current_dir(self.dir.path())
      .args(args)
      .env("GIT_CONFIG_GLOBAL", self.path("gitconfig"))
      .env("GIT_CONFIG_NOSYSTEM", "1")
      .env("GIT_AUTHOR_NAME", "dev")
      .env("GIT_AUTHOR_EMAIL", email)
      .env("GIT_COMMITTER_NAME", "dev")
      .env("GIT_COMMITTER_EMAIL", email)
      .output()
      .unwrap();
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout).unwrap()
  }

  /// Commits `contents` to `file` on top of `branch` (or of `main` when it doesn't exist yet) as `email` & pushes it.
  pub fn commit(&self, branch: &str, email: &str, file: &str, contents: &str) {
    self.git(&["-C", "src", "fetch", "-q", "origin"]);
    if self.tip("main").is_some() {
      let start = if self.tip(branch).is_some() { format!("origin/{branch}") } else { "origin/main".to_string() };
      self.git(&["-C", "src", "checkout", "-q", "-B", branch, &start]);
    }
    std::fs::write(self.path("src").join(file), contents).unwrap();
    self.git(&["-C", "src", "add", "."]);
    self.git_as(email, &["-C", "src", "commit", "-q", "-m", &format!("edit {file}")]);
    self.git(&["-C", "src", "push", "-q", "origin", &format!("+HEAD:refs/heads/{branch}")]);
  }

  /// Creates (or moves) `branch` on the remote to `rev`.
  pub fn branch_at(&self, branch: &str, rev: &str) {
    self.git(&["--git-dir=origin.git", "branch", "-f", branch, rev]);
  }

  pub fn branches(&self) -> Vec<String> {
    let out = self.git(&["--git-dir=origin.git", "for-each-ref", "--format=%(refname:short)", "refs/heads"]);
    out.lines().map(str::to_string).collect()
  }

  pub fn tip(&self, branch: &str) -> Option<String> {
    let rev = format!("refs/heads/{branch}");
    let out = self.remote_git(&["rev-parse", "-q", "--verify", &rev]);
    out.status.success().then(|| String::from_utf8(out.stdout).unwrap().trim().to_string())
  }

  /// The contents of `file` on `branch`, if it exists there.
  pub fn file(&self, branch: &str, file: &str) -> Option<String> {
    let out = self.remote_git(&["show", &format!("{branch}:{file}")]);
    out.status.success().then(|| String::from_utf8(out.stdout).unwrap())
  }

  /// Whether `ancestor` is an ancestor of `branch`.
  pub fn contains(&self, branch: &str, ancestor: &str) -> bool {
    self.remote_git(&["merge-base", "--is-ancestor", ancestor, branch]).status.success()
  }

  fn remote_git(&self, args: &[&str]) -> std::process::Output {
    Command::new("git").current_dir(self.dir.path()).arg("--git-dir=origin.git").args(args).output().unwrap()
  }

  pub fn bot_repo(&self) -> DoupBotRepo {
    DoupBotRepo { origin: self.url.clone(), auth: None }
  }
}

pub fn plugin(reference: &str) -> DoupRepoPlugin {
  DoupRepoPlugin { plugin: reference.to_string(), with: None }
}

pub fn dependency(name: &str, current: &str) -> Dependency {
  Dependency { name: name.to_string(), registry: None, occurrences: Vec::new(), current_version: current.to_string() }
}

pub fn versions(next: &str, latest: Option<&str>) -> Option<Versions> {
  Some(Versions { next: next.to_string(), latest: latest.map(str::to_string) })
}

pub fn note(version: &str) -> ReleaseNote {
  ReleaseNote {
    version: version.to_string(),
    url: format!("https://example.com/{version}"),
    body: format!("Notes for {version}"),
  }
}

fn fake_error(what: &str) -> PluginError {
  PluginError::NotFound { plugin: what.to_string(), searched: Vec::new() }
}

/// A pull request held by [`FakePlatform`].
#[derive(Debug, Clone)]
pub struct FakePr {
  pub number: i64,
  pub head_branch: String,
  pub base_branch: String,
  pub title: String,
  pub body: String,
  pub version: String,
  pub open: bool,
  pub comments: Vec<String>,
}

impl FakePr {
  fn to_pull_request(&self) -> PullRequest {
    PullRequest {
      number: self.number,
      url: format!("https://example.com/pull/{}", self.number),
      head_branch: self.head_branch.clone(),
      base_branch: self.base_branch.clone(),
      version: self.version.clone(),
    }
  }
}

/// A platform that keeps its pull requests in memory.
#[derive(Debug, Default)]
pub struct FakePlatform {
  pub prs: Mutex<Vec<FakePr>>,
  /// Closing these pull requests fails.
  pub fail_close: Mutex<HashSet<i64>>,
}

impl FakePlatform {
  pub fn prs(&self) -> Vec<FakePr> {
    self.prs.lock().unwrap().clone()
  }

  pub fn open_prs(&self) -> Vec<FakePr> {
    self.prs().into_iter().filter(|pr| pr.open).collect()
  }

  pub fn pr(&self, number: i64) -> FakePr {
    self.prs().into_iter().find(|pr| pr.number == number).unwrap()
  }

  /// The open pull request for `branch`.
  pub fn open_pr(&self, branch: &str) -> FakePr {
    self.open_prs().into_iter().find(|pr| pr.head_branch == branch).unwrap()
  }

  /// Adds a pull request, as if opened by a previous run.
  pub fn add_pr(&self, head_branch: &str, version: &str, open: bool) -> i64 {
    let mut prs = self.prs.lock().unwrap();
    let number = prs.len() as i64 + 1;
    prs.push(FakePr {
      number,
      head_branch: head_branch.to_string(),
      base_branch: "main".to_string(),
      title: String::new(),
      body: String::new(),
      version: version.to_string(),
      open,
      comments: Vec::new(),
    });
    number
  }
}

impl Platform for FakePlatform {
  async fn create_pr(&self, head_branch: &str, content: &PullRequestContent) -> Result<PullRequest, PluginError> {
    let number = self.add_pr(head_branch, &content.version, true);
    let mut prs = self.prs.lock().unwrap();
    let pr = prs.iter_mut().find(|pr| pr.number == number).unwrap();
    (pr.base_branch, pr.title, pr.body) = (content.base_branch.clone(), content.title.clone(), content.body.clone());
    Ok(pr.to_pull_request())
  }

  async fn update_pr(&self, number: i64, content: &PullRequestContent) -> Result<PullRequest, PluginError> {
    let mut prs = self.prs.lock().unwrap();
    let pr = prs.iter_mut().find(|pr| pr.number == number && pr.open).ok_or_else(|| fake_error("update_pr"))?;
    (pr.base_branch, pr.title, pr.body) = (content.base_branch.clone(), content.title.clone(), content.body.clone());
    pr.version = content.version.clone();
    Ok(pr.to_pull_request())
  }

  async fn close_pr(&self, number: i64, comment: Option<&str>) -> Result<PullRequest, PluginError> {
    if self.fail_close.lock().unwrap().contains(&number) {
      return Err(fake_error("close_pr"));
    }
    let mut prs = self.prs.lock().unwrap();
    let pr = prs.iter_mut().find(|pr| pr.number == number).ok_or_else(|| fake_error("close_pr"))?;
    pr.open = false;
    pr.comments.extend(comment.map(str::to_string));
    Ok(pr.to_pull_request())
  }

  async fn list_open_prs(&self, head_branch_prefix: &str) -> Result<Vec<PullRequest>, PluginError> {
    let prs = self.prs.lock().unwrap();
    let open = prs.iter().filter(|pr| pr.open && pr.head_branch.starts_with(head_branch_prefix));
    Ok(open.map(FakePr::to_pull_request).collect())
  }

  async fn list_closed_prs(&self, head_branches: &[String]) -> Result<Vec<PullRequest>, PluginError> {
    let prs = self.prs.lock().unwrap();
    let closed = prs.iter().filter(|pr| !pr.open && head_branches.contains(&pr.head_branch));
    Ok(closed.map(FakePr::to_pull_request).collect())
  }
}

/// An ecosystem with a set list of dependencies, that applies an update by writing `<name>.version`.
#[derive(Debug, Default)]
pub struct FakeEcosystem {
  pub dependencies: Mutex<Vec<Dependency>>,
  /// The versions of each dependency, by name. A missing entry makes the lookup fail.
  pub versions: Mutex<HashMap<String, Option<Versions>>>,
  pub notes: Mutex<Vec<ReleaseNote>>,
  /// The `(name, to)` of every dependency release notes were fetched for.
  pub fetched_notes: Mutex<Vec<(String, String)>>,
  /// How long each `apply_updates` call takes.
  pub apply_delay: Mutex<Duration>,
  applying: AtomicUsize,
  pub max_concurrent_applies: AtomicUsize,
}

impl FakeEcosystem {
  /// Adds (or replaces) dependency `name` at `current`, with `versions` available.
  pub fn set(&self, name: &str, current: &str, versions: Option<Versions>) {
    let mut dependencies = self.dependencies.lock().unwrap();
    dependencies.retain(|dependency| dependency.name != name);
    dependencies.push(dependency(name, current));
    self.versions.lock().unwrap().insert(name.to_string(), versions);
  }

  /// Makes looking up `name`'s versions fail.
  pub fn fail_lookup(&self, name: &str) {
    self.versions.lock().unwrap().remove(name);
  }

  pub fn remove(&self, name: &str) {
    self.dependencies.lock().unwrap().retain(|dependency| dependency.name != name);
  }
}

impl Ecosystem for FakeEcosystem {
  async fn discover_deps(&self, _workspace_dir: &Path) -> Result<Vec<Dependency>, PluginError> {
    Ok(self.dependencies.lock().unwrap().clone())
  }

  async fn lookup_versions(&self, dependency: &Dependency) -> Result<Option<Versions>, PluginError> {
    self.versions.lock().unwrap().get(&dependency.name).cloned().ok_or_else(|| fake_error("lookup_versions"))
  }

  async fn fetch_release_notes(&self, dependency: &Dependency, to: &str) -> Result<Vec<ReleaseNote>, PluginError> {
    self.fetched_notes.lock().unwrap().push((dependency.name.clone(), to.to_string()));
    Ok(self.notes.lock().unwrap().clone())
  }

  async fn apply_updates(&self, workspace_dir: &Path, dependency: &Dependency, to: &str) -> Result<(), PluginError> {
    let applying = self.applying.fetch_add(1, Ordering::SeqCst) + 1;
    self.max_concurrent_applies.fetch_max(applying, Ordering::SeqCst);
    let delay = *self.apply_delay.lock().unwrap();
    tokio::time::sleep(delay).await;
    std::fs::write(workspace_dir.join(format!("{}.version", dependency.name)), to).unwrap();
    self.applying.fetch_sub(1, Ordering::SeqCst);
    Ok(())
  }
}

/// Hands out fakes by plugin reference, failing to load any other plugin.
#[derive(Debug, Default, Clone)]
pub struct FakePlugins {
  pub platforms: HashMap<String, Arc<FakePlatform>>,
  pub ecosystems: HashMap<String, Arc<FakeEcosystem>>,
}

impl FakePlugins {
  /// One platform & one ecosystem, as named by [`Remote::new`].
  pub fn single() -> (Self, Arc<FakePlatform>, Arc<FakeEcosystem>) {
    let (platform, ecosystem) = (Arc::new(FakePlatform::default()), Arc::new(FakeEcosystem::default()));
    let mut plugins = Self::default();
    plugins.platforms.insert(PLATFORM.to_string(), platform.clone());
    plugins.ecosystems.insert(ECOSYSTEM.to_string(), ecosystem.clone());
    (plugins, platform, ecosystem)
  }
}

impl PluginLoader for FakePlugins {
  type Platform = Arc<FakePlatform>;
  type Ecosystem = Arc<FakeEcosystem>;

  async fn load_platform(&self, plugin: &DoupRepoPlugin, _: Vec<PathBuf>) -> Result<Self::Platform, PluginError> {
    self.platforms.get(&plugin.plugin).cloned().ok_or_else(|| fake_error(&plugin.plugin))
  }

  async fn load_ecosystem(&self, plugin: &DoupRepoPlugin, _: Vec<PathBuf>) -> Result<Self::Ecosystem, PluginError> {
    self.ecosystems.get(&plugin.plugin).cloned().ok_or_else(|| fake_error(&plugin.plugin))
  }
}

/// An updater for `repos` using `git` & `plugins`.
pub fn updater(git: Git, repos: Vec<DoupBotRepo>, plugins: &FakePlugins) -> Updater<FakePlugins> {
  Updater::new(git, repos).plugins(plugins.clone())
}

pub fn concurrency(repos: usize, plugin_calls: usize, apply_updates: usize) -> Concurrency {
  Concurrency {
    repos: NonZeroUsize::new(repos).unwrap(),
    plugin_calls: NonZeroUsize::new(plugin_calls).unwrap(),
    apply_updates: NonZeroUsize::new(apply_updates).unwrap(),
  }
}

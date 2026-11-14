//! Clones from real remotes, so these need network access.

use git::{Auth, CloneOptions, Git, GitBackend, GitCli, Grit};
use std::num::NonZeroU32;
use tempfile::TempDir;

async fn clones_a_public_https_repository(backend: impl Into<Git>) {
  let dir = TempDir::new().unwrap();
  let dest = dir.path().join("clone");
  let options = CloneOptions::new("https://github.com/octocat/Hello-World.git", &dest)
    .with_depth(NonZeroU32::new(1).unwrap());
  Git::new(backend).clone_repo(&options).await.unwrap();
  assert!(dest.join(".git").is_dir());
  assert!(dest.join("README").exists());
}

const SSH_SKIPPED: &str = "set DOUP_TEST_SSH_URL and DOUP_TEST_SSH_KEY to run";

/// The private repo and its read-only deploy key, which only CI and opted-in devs have.
fn ssh_remote() -> (String, String) {
  let var = |name| std::env::var(name).unwrap_or_else(|_| panic!("{SSH_SKIPPED}"));
  (var("DOUP_TEST_SSH_URL"), var("DOUP_TEST_SSH_KEY"))
}

async fn clones_over_ssh_with_an_in_memory_key(backend: impl Into<Git>) {
  let (url, key) = ssh_remote();
  let dir = TempDir::new().unwrap();
  let dest = dir.path().join("clone");
  let options = CloneOptions::new(url, &dest)
    .with_depth(NonZeroU32::new(1).unwrap())
    .with_auth(Auth::Ssh { key });
  Git::new(backend).clone_repo(&options).await.unwrap();
  assert!(dest.join(".git").is_dir());
  assert!(dest.join("README.md").exists());
}

#[tokio::test]
async fn git_cli_clones_a_public_https_repository() {
  clones_a_public_https_repository(GitCli).await;
}

#[tokio::test]
async fn grit_clones_a_public_https_repository() {
  clones_a_public_https_repository(Grit).await;
}

#[tokio::test]
#[cfg_attr(
  not(doup_test_ssh),
  ignore = "set DOUP_TEST_SSH_URL and DOUP_TEST_SSH_KEY to run"
)]
async fn git_cli_clones_over_ssh_with_an_in_memory_key() {
  clones_over_ssh_with_an_in_memory_key(GitCli).await;
}

#[tokio::test]
#[cfg_attr(
  not(doup_test_ssh),
  ignore = "set DOUP_TEST_SSH_URL and DOUP_TEST_SSH_KEY to run"
)]
async fn grit_clones_over_ssh_with_an_in_memory_key() {
  clones_over_ssh_with_an_in_memory_key(Grit).await;
}

#[test]
fn ssh_tests_are_not_ignored_in_ci() {
  if std::env::var_os("CI").is_some() && !cfg!(doup_test_ssh) {
    panic!("{SSH_SKIPPED}");
  }
}

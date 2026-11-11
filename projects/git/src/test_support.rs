use std::path::{Path, PathBuf};
use std::process::Command;
use tempfile::TempDir;

/// A bare repository on disk to clone from, served over `file://` so `--depth` is honored.
///
/// `main` has two commits (README is "two"); `feature` adds `feature.txt` on top.
pub(crate) struct Fixture {
  dir: TempDir,
  pub(crate) url: String,
}

impl Fixture {
  pub(crate) fn new() -> Self {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    // Isolate from the developer's config (e.g. commit signing).
    let global = root.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    let git = |cwd: &Path, args: &[&str]| {
      let output = Command::new("git")
        .current_dir(cwd)
        .args(args)
        .env("GIT_CONFIG_GLOBAL", &global)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("GIT_AUTHOR_NAME", "doup")
        .env("GIT_AUTHOR_EMAIL", "doup@example.com")
        .env("GIT_COMMITTER_NAME", "doup")
        .env("GIT_COMMITTER_EMAIL", "doup@example.com")
        .output()
        .unwrap();
      assert!(output.status.success(), "git {args:?}: {output:?}");
    };

    let src = root.join("src");
    std::fs::create_dir(&src).unwrap();
    git(&src, &["init", "-q", "-b", "main"]);
    std::fs::write(src.join("README"), "one\n").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "-q", "-m", "one"]);
    std::fs::write(src.join("README"), "two\n").unwrap();
    git(&src, &["commit", "-q", "-am", "two"]);
    git(&src, &["checkout", "-q", "-b", "feature"]);
    std::fs::write(src.join("feature.txt"), "feature\n").unwrap();
    git(&src, &["add", "."]);
    git(&src, &["commit", "-q", "-m", "feature"]);
    git(&src, &["checkout", "-q", "main"]);
    git(root, &["clone", "-q", "--bare", "src", "origin.git"]);

    let url = file_url(&root.join("origin.git"));
    Self { dir, url }
  }

  pub(crate) fn path(&self, name: &str) -> PathBuf {
    self.dir.path().join(name)
  }
}

pub(crate) fn file_url(path: &Path) -> String {
  url::Url::from_file_path(path).unwrap().to_string()
}

/// A fresh OpenSSH private key of `kind` (an `ssh-keygen -t` value), encrypted if `passphrase` is set.
pub(crate) fn generate_ssh_key(kind: &str, passphrase: &str) -> String {
  let dir = TempDir::new().unwrap();
  let path = dir.path().join("key");
  let output = Command::new("ssh-keygen")
    .args(["-q", "-t", kind, "-N", passphrase, "-C", "doup-test", "-f"])
    .arg(&path)
    .output()
    .unwrap();
  assert!(output.status.success(), "ssh-keygen -t {kind}: {output:?}");
  std::fs::read_to_string(path).unwrap()
}

/// A check that an error is the variant a test expects.
pub(crate) type Expect = fn(&crate::GitError) -> bool;

/// The branch a clone has checked out, read straight from `.git/HEAD`.
pub(crate) fn head_branch(repo: &Path) -> String {
  let head = std::fs::read_to_string(repo.join(".git/HEAD")).unwrap();
  head
    .trim()
    .strip_prefix("ref: refs/heads/")
    .unwrap()
    .to_owned()
}

pub(crate) fn commit_count(repo: &Path) -> u32 {
  let output = Command::new("git")
    .current_dir(repo)
    .args(["rev-list", "--count", "HEAD"])
    .output()
    .unwrap();
  assert!(output.status.success(), "{output:?}");
  String::from_utf8(output.stdout)
    .unwrap()
    .trim()
    .parse()
    .unwrap()
}

/// Behaviour every [`crate::GitBackend`] must share, generated as tests for `$backend`.
macro_rules! backend_contract_tests {
  ($backend:expr) => {
    mod contract {
      use crate::test_support::{Fixture, commit_count, file_url, head_branch};
      use crate::{CloneOptions, Git, GitError};
      use std::num::NonZeroU32;

      async fn clone(options: CloneOptions) -> Result<(), GitError> {
        Git::new($backend).clone_repo(options).await
      }

      #[tokio::test]
      async fn clones_the_default_branch_with_full_history() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        clone(CloneOptions::new(&fixture.url, &dest)).await.unwrap();
        assert_eq!(std::fs::read_to_string(dest.join("README")).unwrap(), "two\n");
        assert_eq!(head_branch(&dest), "main");
        assert!(!dest.join("feature.txt").exists());
        assert_eq!(commit_count(&dest), 2);
      }

      #[tokio::test]
      async fn clones_a_named_branch() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        clone(CloneOptions::new(&fixture.url, &dest).with_branch("feature"))
          .await
          .unwrap();
        assert_eq!(head_branch(&dest), "feature");
        assert!(dest.join("feature.txt").exists());
      }

      #[tokio::test]
      async fn makes_a_shallow_clone() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        let depth = NonZeroU32::new(1).unwrap();
        clone(CloneOptions::new(&fixture.url, &dest).with_depth(depth))
          .await
          .unwrap();
        assert!(dest.join(".git/shallow").exists());
        assert_eq!(commit_count(&dest), 1);
      }

      #[tokio::test]
      async fn clones_into_an_existing_empty_directory() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        std::fs::create_dir(&dest).unwrap();
        clone(CloneOptions::new(&fixture.url, &dest)).await.unwrap();
        assert_eq!(head_branch(&dest), "main");
      }

      #[tokio::test]
      async fn reports_a_missing_repository() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        let url = file_url(&fixture.path("missing.git"));
        let err = clone(CloneOptions::new(url, &dest)).await.unwrap_err();
        assert!(
          matches!(err, GitError::RepositoryNotFound { .. }),
          "{err:?}"
        );
        assert!(!dest.exists());
      }

      #[tokio::test]
      async fn reports_an_unknown_branch_and_cleans_up() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        let err = clone(CloneOptions::new(&fixture.url, &dest).with_branch("nope"))
          .await
          .unwrap_err();
        assert!(
          matches!(&err, GitError::BranchNotFound { branch } if branch == "nope"),
          "{err:?}"
        );
        assert!(!dest.exists());
      }

      #[tokio::test]
      async fn keeps_an_existing_destination_empty_after_a_failure() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        std::fs::create_dir(&dest).unwrap();
        let err = clone(CloneOptions::new(&fixture.url, &dest).with_branch("nope"))
          .await
          .unwrap_err();
        assert!(matches!(err, GitError::BranchNotFound { .. }), "{err:?}");
        assert_eq!(std::fs::read_dir(&dest).unwrap().count(), 0);
      }
    }
  };
}

pub(crate) use backend_contract_tests;

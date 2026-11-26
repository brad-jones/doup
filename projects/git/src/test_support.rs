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

/// Runs git in `cwd`, isolated from the developer's config (e.g. commit signing) by `global`.
fn git(global: &Path, cwd: &Path, args: &[&str]) {
  git_as(global, cwd, "doup@example.com", args);
}

/// Like [`git`], with commits authored by `email`.
fn git_as(global: &Path, cwd: &Path, email: &str, args: &[&str]) {
  let output = git_output(global, cwd, email, args);
  assert!(output.status.success(), "git {args:?}: {output:?}");
}

fn git_output(global: &Path, cwd: &Path, email: &str, args: &[&str]) -> std::process::Output {
  Command::new("git")
    .current_dir(cwd)
    .args(args)
    .env("GIT_CONFIG_GLOBAL", global)
    .env("GIT_CONFIG_NOSYSTEM", "1")
    .env("GIT_AUTHOR_NAME", "doup")
    .env("GIT_AUTHOR_EMAIL", email)
    .env("GIT_COMMITTER_NAME", "doup")
    .env("GIT_COMMITTER_EMAIL", "doup@example.com")
    .output()
    .unwrap()
}

impl Fixture {
  pub(crate) fn new() -> Self {
    let dir = TempDir::new().unwrap();
    let root = dir.path();
    let global = root.join("gitconfig");
    std::fs::write(&global, "").unwrap();
    let git = |cwd: &Path, args: &[&str]| git(&global, cwd, args);

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
    git(root, &["--git-dir=origin.git", "config", "uploadpack.allowFilter", "true"]);

    let url = file_url(&root.join("origin.git"));
    Self { dir, url }
  }

  pub(crate) fn path(&self, name: &str) -> PathBuf {
    self.dir.path().join(name)
  }

  /// Creates `branch` on the remote, pointing at `main`.
  pub(crate) fn add_branch(&self, branch: &str) {
    let git_dir = format!("--git-dir={}", self.path("origin.git").display());
    git(&self.path("gitconfig"), self.dir.path(), &[&git_dir, "branch", branch, "main"]);
  }

  /// Commits on top of `branch` on the remote as `author`, creating `branch` from `main` if needed.
  pub(crate) fn commit(&self, branch: &str, author: &str, message: &str) {
    self.in_src(branch, author, &["commit", "-q", "--allow-empty", "-m", message]);
  }

  /// Merges `main` into `branch` on the remote as `author`.
  pub(crate) fn merge_main_into(&self, branch: &str, author: &str) {
    self.in_src(branch, author, &["merge", "-q", "--no-ff", "--no-edit", "main"]);
  }

  /// Runs `args` in the source repo on `branch`, then pushes it to the remote.
  fn in_src(&self, branch: &str, author: &str, args: &[&str]) {
    let (global, src) = (self.path("gitconfig"), self.path("src"));
    let origin = self.path("origin.git");
    let origin = origin.to_str().unwrap();
    let start = format!("refs/heads/{branch}");
    git(&global, &src, &["checkout", "-q", "--detach"]);
    git(&global, &src, &["fetch", "-q", "--prune", origin, "+refs/heads/*:refs/heads/*"]);
    let exists =
      git_output(&global, &src, "doup@example.com", &["rev-parse", "-q", "--verify", &start]).status.success();
    git(&global, &src, &["checkout", "-q", "-B", branch, if exists { &start } else { "refs/heads/main" }]);
    git_as(&global, &src, author, args);
    git(&global, &src, &["push", "-q", origin, &format!("+{branch}:{branch}")]);
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
  head.trim().strip_prefix("ref: refs/heads/").unwrap().to_owned()
}

pub(crate) fn commit_count(repo: &Path) -> u32 {
  let output = Command::new("git").current_dir(repo).args(["rev-list", "--count", "HEAD"]).output().unwrap();
  assert!(output.status.success(), "{output:?}");
  String::from_utf8(output.stdout).unwrap().trim().parse().unwrap()
}

/// Behaviour every [`crate::GitBackend`] must share, generated as tests for `$backend`.
macro_rules! backend_contract_tests {
  ($backend:expr) => {
    mod contract {
      use crate::test_support::{Fixture, commit_count, file_url, head_branch};
      use crate::{CloneOptions, ClonedRepo, Git, GitCommit, GitError};
      use std::num::NonZeroU32;

      async fn clone(options: CloneOptions) -> Result<ClonedRepo, GitError> {
        Git::new($backend).clone_repo(&options).await
      }

      #[tokio::test]
      async fn lists_remote_branches_sorted() {
        let fixture = Fixture::new();
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone"))).await.unwrap();
        assert_eq!(repo.list_remote_branches().await.unwrap(), ["feature", "main"]);
      }

      #[tokio::test]
      async fn lists_branches_a_shallow_single_branch_clone_did_not_fetch() {
        let fixture = Fixture::new();
        let depth = NonZeroU32::new(1).unwrap();
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone")).with_depth(depth))
          .await
          .unwrap();
        assert_eq!(repo.list_remote_branches().await.unwrap(), ["feature", "main"]);
      }

      #[tokio::test]
      async fn lists_branches_created_on_the_remote_after_cloning() {
        let fixture = Fixture::new();
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone"))).await.unwrap();
        fixture.add_branch("later");
        assert_eq!(repo.list_remote_branches().await.unwrap(), ["feature", "later", "main"]);
      }

      #[tokio::test]
      async fn reports_a_remote_that_has_gone_away() {
        let fixture = Fixture::new();
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone"))).await.unwrap();
        std::fs::remove_dir_all(fixture.path("origin.git")).unwrap();
        let err = repo.list_remote_branches().await.unwrap_err();
        assert!(matches!(err, GitError::RepositoryNotFound { .. }), "{err:?}");
      }

      async fn shallow_clone(fixture: &Fixture) -> ClonedRepo {
        let depth = NonZeroU32::new(1).unwrap();
        clone(CloneOptions::new(&fixture.url, fixture.path("clone")).with_depth(depth))
          .await
          .unwrap()
      }

      fn messages(commits: &[GitCommit]) -> Vec<&str> {
        commits.iter().map(GitCommit::message).collect()
      }

      #[tokio::test]
      async fn reads_the_default_branch() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        assert_eq!(repo.default_branch().await.unwrap(), "main");
      }

      #[tokio::test]
      async fn lists_only_a_branchs_own_commits_from_a_shallow_clone() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let commits = repo.branch_commits("feature").await.unwrap();
        assert_eq!(messages(&commits), ["feature\n"]);
        let commit = &commits[0];
        assert_eq!((commit.author_name(), commit.author_email()), ("doup", "doup@example.com"));
        assert_eq!(commit.id().len(), 40);
        assert!(!commit.is_merge());
        assert_eq!(repo.branch_authors("feature").await.unwrap(), ["doup@example.com"]);
      }

      #[tokio::test]
      async fn lists_only_a_branchs_own_commits_from_a_full_clone() {
        let fixture = Fixture::new();
        let dest = fixture.path("clone");
        let repo = clone(CloneOptions::new(&fixture.url, &dest)).await.unwrap();
        assert_eq!(messages(&repo.branch_commits("feature").await.unwrap()), ["feature\n"]);
        // The default branch's own history must not be cut short.
        assert_eq!(commit_count(&dest), 2);
      }

      #[tokio::test]
      async fn lists_authors_sorted_and_deduplicated() {
        let fixture = Fixture::new();
        fixture.commit("feature", "zed@example.com", "z1");
        fixture.commit("feature", "amy@example.com", "a1");
        fixture.commit("feature", "zed@example.com", "z2");
        let repo = shallow_clone(&fixture).await;
        assert_eq!(messages(&repo.branch_commits("feature").await.unwrap()), ["z2\n", "a1\n", "z1\n", "feature\n"]);
        assert_eq!(
          repo.branch_authors("feature").await.unwrap(),
          ["amy@example.com", "doup@example.com", "zed@example.com"]
        );
      }

      #[tokio::test]
      async fn ignores_commits_added_to_the_default_branch_later() {
        let fixture = Fixture::new();
        fixture.commit("main", "late@example.com", "late");
        let repo = shallow_clone(&fixture).await;
        assert_eq!(repo.branch_authors("feature").await.unwrap(), ["doup@example.com"]);
      }

      #[tokio::test]
      async fn fetches_branches_created_or_moved_after_cloning() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        assert_eq!(repo.branch_commits("feature").await.unwrap().len(), 1);
        fixture.commit("feature", "amy@example.com", "moved");
        fixture.commit("later", "bob@example.com", "created");
        assert_eq!(messages(&repo.branch_commits("feature").await.unwrap()), ["moved\n", "feature\n"]);
        assert_eq!(repo.branch_authors("later").await.unwrap(), ["bob@example.com"]);
      }

      #[tokio::test]
      async fn stops_at_a_merge_from_the_default_branch() {
        let fixture = Fixture::new();
        fixture.commit("main", "late@example.com", "late");
        fixture.merge_main_into("feature", "merger@example.com");
        fixture.commit("feature", "amy@example.com", "after");
        let repo = shallow_clone(&fixture).await;
        let commits = repo.branch_commits("feature").await.unwrap();
        assert_eq!(commits.len(), 2, "{commits:?}");
        assert_eq!(commits[0].message(), "after\n");
        assert!(!commits[0].is_merge());
        assert_eq!(commits[1].author_email(), "merger@example.com");
        assert!(commits[1].is_merge());
      }

      #[tokio::test]
      async fn a_branch_at_the_default_branchs_tip_has_no_commits() {
        let fixture = Fixture::new();
        fixture.add_branch("same");
        let repo = shallow_clone(&fixture).await;
        for branch in ["main", "same"] {
          assert_eq!(repo.branch_commits(branch).await.unwrap(), [], "{branch}");
          assert_eq!(repo.branch_authors(branch).await.unwrap(), Vec::<String>::new(), "{branch}");
          repo.branch_fetch(branch).await.unwrap();
        }
      }

      #[tokio::test]
      async fn reports_a_branch_behind_the_default_branch() {
        let fixture = Fixture::new();
        fixture.add_branch("merged");
        fixture.commit("main", "late@example.com", "late");
        let repo = shallow_clone(&fixture).await;
        let err = repo.branch_commits("merged").await.unwrap_err();
        assert!(matches!(&err, GitError::NoUniqueCommits { branch } if branch == "merged"), "{err:?}");
      }

      #[tokio::test]
      async fn reports_unknown_and_blank_branches() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let err = repo.branch_commits("nope").await.unwrap_err();
        assert!(matches!(&err, GitError::BranchNotFound { branch } if branch == "nope"), "{err:?}");
        let err = repo.branch_fetch(" ").await.unwrap_err();
        assert!(matches!(err, GitError::EmptyBranch), "{err:?}");
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

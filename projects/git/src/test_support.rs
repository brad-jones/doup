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

  /// The full id `branch` points at on the remote, if it exists.
  pub(crate) fn tip(&self, branch: &str) -> Option<String> {
    let tip = format!("refs/heads/{branch}");
    let output = self.remote_git(&["rev-parse", "-q", "--verify", &tip]);
    output.status.success().then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
  }

  /// `git show -s --format=<format> <rev>` on the remote.
  pub(crate) fn show(&self, rev: &str, format: &str) -> String {
    let output = self.remote_git(&["show", "-s", &format!("--format={format}"), rev]);
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
  }

  /// Every file in `rev`'s tree on the remote, with its contents.
  pub(crate) fn files(&self, rev: &str) -> Vec<(String, String)> {
    let output = self.remote_git(&["ls-tree", "-r", "--name-only", rev]);
    assert!(output.status.success(), "{output:?}");
    let names = String::from_utf8(output.stdout).unwrap();
    names
      .lines()
      .map(|name| {
        let blob = self.remote_git(&["cat-file", "blob", &format!("{rev}:{name}")]);
        (name.to_owned(), String::from_utf8(blob.stdout).unwrap())
      })
      .collect()
  }

  fn remote_git(&self, args: &[&str]) -> std::process::Output {
    let git_dir = format!("--git-dir={}", self.path("origin.git").display());
    let args: Vec<&str> = std::iter::once(git_dir.as_str()).chain(args.iter().copied()).collect();
    git_output(&self.path("gitconfig"), self.dir.path(), "doup@example.com", &args)
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

/// The worktrees registered in the clone at `repo`, its own included.
pub(crate) fn worktree_count(repo: &Path) -> usize {
  let output = Command::new("git").current_dir(repo).args(["worktree", "list", "--porcelain"]).output().unwrap();
  assert!(output.status.success(), "{output:?}");
  String::from_utf8(output.stdout).unwrap().lines().filter(|line| line.starts_with("worktree ")).count()
}

/// The branch checked out in a working tree, which may be a linked worktree.
pub(crate) fn checked_out(path: &Path) -> String {
  let output = Command::new("git").current_dir(path).args(["symbolic-ref", "--short", "HEAD"]).output().unwrap();
  assert!(output.status.success(), "{output:?}");
  String::from_utf8(output.stdout).unwrap().trim().to_owned()
}

/// Behaviour every [`crate::GitBackend`] must share, generated as tests for `$backend`.
macro_rules! backend_contract_tests {
  ($backend:expr) => {
    mod contract {
      use crate::test_support::{Fixture, checked_out, commit_count, file_url, head_branch, worktree_count};
      use crate::{CloneOptions, ClonedRepo, Git, GitCommit, GitError, Identity, Worktree};
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
      async fn a_branch_on_the_default_branchs_tip_is_not_behind() {
        let fixture = Fixture::new();
        fixture.add_branch("same");
        fixture.commit("feature", "amy@example.com", "more");
        let repo = shallow_clone(&fixture).await;
        for branch in ["main", "same", "feature"] {
          assert!(!repo.branch_is_behind(branch).await.unwrap(), "{branch}");
        }
      }

      #[tokio::test]
      async fn a_branch_is_behind_once_the_default_branch_moves_on() {
        let fixture = Fixture::new();
        fixture.add_branch("merged");
        fixture.commit("main", "late@example.com", "late");
        let repo = shallow_clone(&fixture).await;
        assert!(repo.branch_is_behind("feature").await.unwrap());
        // Behind, with no commits of its own.
        assert!(repo.branch_is_behind("merged").await.unwrap());
      }

      #[tokio::test]
      async fn a_branch_with_the_default_branch_merged_in_is_not_behind() {
        let fixture = Fixture::new();
        fixture.commit("main", "late@example.com", "late");
        fixture.merge_main_into("feature", "merger@example.com");
        fixture.commit("feature", "amy@example.com", "after");
        let repo = shallow_clone(&fixture).await;
        assert!(!repo.branch_is_behind("feature").await.unwrap());
        fixture.commit("main", "later@example.com", "later");
        assert!(repo.branch_is_behind("feature").await.unwrap());
      }

      #[tokio::test]
      async fn reports_a_branchs_status_with_one_fetch() {
        let fixture = Fixture::new();
        fixture.add_branch("merged");
        fixture.commit("feature", "amy@example.com", "more");
        fixture.commit("main", "late@example.com", "late");
        let repo = shallow_clone(&fixture).await;

        let status = repo.branch_status("feature").await.unwrap();
        assert_eq!(status.authors(), ["amy@example.com", "doup@example.com"]);
        assert!(status.is_behind());

        // Unlike `branch_authors`, a branch with no commits of its own is not an error.
        let status = repo.branch_status("merged").await.unwrap();
        assert!(status.authors().is_empty());
        assert!(status.is_behind());

        let status = repo.branch_status("main").await.unwrap();
        assert!(status.authors().is_empty());
        assert!(!status.is_behind());
      }

      #[tokio::test]
      async fn an_unknown_branch_can_not_be_behind() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let err = repo.branch_is_behind("nope").await.unwrap_err();
        assert!(matches!(&err, GitError::BranchNotFound { branch } if branch == "nope"), "{err:?}");
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

      const BRANCH: &str = "doup/cargo/serde";

      fn write(worktree: &Worktree, name: &str, contents: &str) {
        std::fs::write(worktree.path().join(name), contents).unwrap();
      }

      #[tokio::test]
      async fn adds_a_worktree_on_a_new_branch_at_the_clones_head() {
        let fixture = Fixture::new();
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone")).with_branch("feature")).await.unwrap();
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        assert_ne!(worktree.path(), repo.path());
        assert_eq!(worktree.branch(), BRANCH);
        assert_eq!(checked_out(worktree.path()), BRANCH);
        assert!(worktree.path().join("feature.txt").exists());
        assert_eq!(head_branch(repo.path()), "feature");
        assert_eq!(worktree_count(repo.path()), 2);

        let path = worktree.path().to_path_buf();
        worktree.remove().await.unwrap();
        assert!(!path.exists());
        assert_eq!(worktree_count(repo.path()), 1);
        assert_eq!(fixture.tip(BRANCH), None, "nothing is pushed until asked");
      }

      #[tokio::test]
      async fn commits_every_change_and_pushes_a_new_branch() {
        let fixture = Fixture::new();
        let depth = NonZeroU32::new(1).unwrap();
        let options = CloneOptions::new(&fixture.url, fixture.path("clone")).with_branch("feature").with_depth(depth);
        let repo = clone(options).await.unwrap();
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        write(&worktree, "README", "three\n");
        write(&worktree, "new.txt", "new\n");
        std::fs::remove_file(worktree.path().join("feature.txt")).unwrap();

        let id = worktree.commit("chore(doup): update serde").await.unwrap();
        worktree.push().await.unwrap();
        assert_eq!(fixture.tip(BRANCH).as_deref(), Some(id.as_str()));
        assert_eq!(
          fixture.files(&id),
          [("README".to_owned(), "three\n".to_owned()), ("new.txt".to_owned(), "new\n".to_owned())]
        );
        assert_eq!(
          fixture.show(&id, "%an <%ae>%n%cn <%ce>%n%P%n%B"),
          format!(
            "doup[bot] <doup[bot]@users.noreply.github.com>\ndoup[bot] <doup[bot]@users.noreply.github.com>\n{}\nchore(doup): update serde\n\n",
            fixture.tip("feature").unwrap()
          )
        );
      }

      #[tokio::test]
      async fn commits_as_the_configured_identity() {
        let fixture = Fixture::new();
        let identity = Identity::new("Jane Doe", "jane@example.com").unwrap();
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone")).with_identity(identity)).await.unwrap();
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        write(&worktree, "README", "three\n");
        let id = worktree.commit("x").await.unwrap();
        worktree.push().await.unwrap();
        assert_eq!(fixture.show(&id, "%an <%ae> %cn <%ce>"), "Jane Doe <jane@example.com> Jane Doe <jane@example.com>\n");
      }

      #[tokio::test]
      async fn refuses_to_commit_nothing() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        let err = worktree.commit("x").await.unwrap_err();
        assert!(matches!(err, GitError::NothingToCommit), "{err:?}");
        write(&worktree, "README", "three\n");
        let err = worktree.commit(" \n").await.unwrap_err();
        assert!(matches!(err, GitError::EmptyCommitMessage), "{err:?}");
      }

      #[tokio::test]
      async fn rejects_invalid_branch_names() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let err = repo.worktree_add(" ").await.unwrap_err();
        assert!(matches!(err, GitError::EmptyBranch), "{err:?}");
        for branch in ["a..b", "-x", "a b", "x.lock"] {
          let err = repo.worktree_add(branch).await.unwrap_err();
          assert!(matches!(&err, GitError::InvalidBranch { branch: b } if b == branch), "{err:?}");
        }
      }

      #[tokio::test]
      async fn push_does_not_overwrite_but_push_force_does() {
        let fixture = Fixture::new();
        fixture.commit(BRANCH, "someone@example.com", "theirs");
        let theirs = fixture.tip(BRANCH).unwrap();
        let repo = shallow_clone(&fixture).await;
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        write(&worktree, "README", "three\n");
        let id = worktree.commit("ours").await.unwrap();

        let err = worktree.push().await.unwrap_err();
        assert!(matches!(&err, GitError::PushRejected { branch, .. } if branch == BRANCH), "{err:?}");
        assert_eq!(fixture.tip(BRANCH), Some(theirs));

        worktree.push_force().await.unwrap();
        assert_eq!(fixture.tip(BRANCH), Some(id));
      }

      #[tokio::test]
      async fn push_force_refuses_a_branch_moved_since_the_worktree_was_added() {
        let fixture = Fixture::new();
        fixture.commit(BRANCH, "someone@example.com", "theirs");
        let repo = shallow_clone(&fixture).await;
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        fixture.commit(BRANCH, "someone@example.com", "moved");
        let moved = fixture.tip(BRANCH);
        write(&worktree, "README", "three\n");
        worktree.commit("ours").await.unwrap();
        let err = worktree.push_force().await.unwrap_err();
        assert!(matches!(&err, GitError::StaleLease { branch } if branch == BRANCH), "{err:?}");
        assert_eq!(fixture.tip(BRANCH), moved);
      }

      #[tokio::test]
      async fn push_force_refuses_a_branch_created_since_the_worktree_was_added() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        fixture.commit(BRANCH, "someone@example.com", "created");
        let created = fixture.tip(BRANCH);
        write(&worktree, "README", "three\n");
        worktree.commit("ours").await.unwrap();
        let err = worktree.push_force().await.unwrap_err();
        assert!(matches!(err, GitError::StaleLease { .. }), "{err:?}");
        assert_eq!(fixture.tip(BRANCH), created);
      }

      #[tokio::test]
      async fn the_lease_follows_each_push() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        write(&worktree, "README", "three\n");
        worktree.commit("first").await.unwrap();
        worktree.push().await.unwrap();
        write(&worktree, "README", "four\n");
        let second = worktree.commit("second").await.unwrap();
        worktree.push_force().await.unwrap();
        assert_eq!(fixture.tip(BRANCH).as_deref(), Some(second.as_str()));
        worktree.remove().await.unwrap();

        // A new worktree resets the branch to HEAD, so this is the doup update case.
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        assert_eq!(std::fs::read_to_string(worktree.path().join("README")).unwrap(), "two\n");
        write(&worktree, "README", "five\n");
        let third = worktree.commit("third").await.unwrap();
        worktree.push_force().await.unwrap();
        assert_eq!(fixture.tip(BRANCH), Some(third));
        assert_eq!(fixture.show(BRANCH, "%P"), format!("{}\n", fixture.tip("main").unwrap()));
        // Commits doup pushed are recognised as doup's own.
        assert_eq!(repo.branch_authors(BRANCH).await.unwrap(), [repo.identity().email()]);
      }

      #[cfg(unix)]
      #[tokio::test]
      async fn never_runs_hooks() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let (hooks, ran) = (repo.path().join(".git/hooks"), fixture.path("hook-ran"));
        for hook in ["post-checkout", "pre-commit", "prepare-commit-msg", "commit-msg", "post-commit", "pre-push"] {
          let path = hooks.join(hook);
          std::fs::write(&path, format!("#!/bin/sh\necho {hook} >> '{}'\nexit 1\n", ran.display())).unwrap();
          std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        write(&worktree, "README", "three\n");
        worktree.commit("x").await.unwrap();
        worktree.push().await.unwrap();
        assert!(!ran.exists(), "{}", std::fs::read_to_string(&ran).unwrap());
      }

      #[tokio::test]
      async fn dropping_a_worktree_removes_it() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let worktree = repo.worktree_add(BRANCH).await.unwrap();
        write(&worktree, "untracked.txt", "x\n");
        let path = worktree.path().to_path_buf();
        drop(worktree);
        assert!(!path.exists());
        assert_eq!(worktree_count(repo.path()), 1);
        // The branch is no longer checked out anywhere, so can be again.
        repo.worktree_add(BRANCH).await.unwrap().remove().await.unwrap();
      }

      fn tracking_tip(repo: &ClonedRepo, branch: &str) -> Option<String> {
        let tracking = format!("refs/remotes/origin/{branch}");
        let output = std::process::Command::new("git")
          .current_dir(repo.path())
          .args(["rev-parse", "-q", "--verify", &tracking])
          .output()
          .unwrap();
        output.status.success().then(|| String::from_utf8(output.stdout).unwrap().trim().to_owned())
      }

      #[tokio::test]
      async fn deletes_a_fetched_branch() {
        let fixture = Fixture::new();
        fixture.commit(BRANCH, "doup[bot]@users.noreply.github.com", "ours");
        let repo = shallow_clone(&fixture).await;
        // Vetting fetches the branch, so it becomes the lease.
        repo.branch_authors(BRANCH).await.unwrap();
        assert_eq!(tracking_tip(&repo, BRANCH), fixture.tip(BRANCH));

        repo.delete_remote_branch(BRANCH).await.unwrap();
        assert_eq!(fixture.tip(BRANCH), None);
        assert_eq!(tracking_tip(&repo, BRANCH), None);
        assert_eq!(repo.list_remote_branches().await.unwrap(), ["feature", "main"]);
      }

      #[tokio::test]
      async fn deletes_a_branch_that_was_never_fetched() {
        let fixture = Fixture::new();
        fixture.add_branch(BRANCH);
        let repo = clone(CloneOptions::new(&fixture.url, fixture.path("clone"))).await.unwrap();
        repo.delete_remote_branch("feature").await.unwrap();
        repo.delete_remote_branch(BRANCH).await.unwrap();
        assert_eq!((fixture.tip("feature"), fixture.tip(BRANCH)), (None, None));
        assert_eq!(tracking_tip(&repo, "feature"), None, "a full clone's tracking ref is deleted too");
        assert_eq!(repo.list_remote_branches().await.unwrap(), ["main"]);
      }

      #[tokio::test]
      async fn refuses_to_delete_a_branch_moved_since_it_was_fetched() {
        let fixture = Fixture::new();
        fixture.commit(BRANCH, "doup[bot]@users.noreply.github.com", "ours");
        let repo = shallow_clone(&fixture).await;
        repo.branch_authors(BRANCH).await.unwrap();
        fixture.commit(BRANCH, "someone@example.com", "theirs");
        let moved = fixture.tip(BRANCH);

        let err = repo.delete_remote_branch(BRANCH).await.unwrap_err();
        assert!(matches!(&err, GitError::StaleLease { branch } if branch == BRANCH), "{err:?}");
        assert_eq!(fixture.tip(BRANCH), moved);
      }

      #[tokio::test]
      async fn deleting_a_missing_branch_is_not_an_error() {
        let fixture = Fixture::new();
        fixture.commit(BRANCH, "doup[bot]@users.noreply.github.com", "ours");
        let repo = shallow_clone(&fixture).await;
        repo.delete_remote_branch("doup/never/existed").await.unwrap();

        // Fetched, then deleted by someone else.
        repo.branch_authors(BRANCH).await.unwrap();
        let other = clone(CloneOptions::new(&fixture.url, fixture.path("other"))).await.unwrap();
        other.delete_remote_branch(BRANCH).await.unwrap();
        repo.delete_remote_branch(BRANCH).await.unwrap();
        assert_eq!(fixture.tip(BRANCH), None);
      }

      #[tokio::test]
      async fn refuses_to_delete_the_default_branch() {
        let fixture = Fixture::new();
        let main = fixture.tip("main");
        let repo = shallow_clone(&fixture).await;
        let err = repo.delete_remote_branch("main").await.unwrap_err();
        assert!(matches!(&err, GitError::DeleteDefaultBranch { branch } if branch == "main"), "{err:?}");
        assert_eq!(fixture.tip("main"), main);
      }

      #[tokio::test]
      async fn refuses_to_delete_invalid_branch_names() {
        let fixture = Fixture::new();
        let repo = shallow_clone(&fixture).await;
        let err = repo.delete_remote_branch(" ").await.unwrap_err();
        assert!(matches!(err, GitError::EmptyBranch), "{err:?}");
        let err = repo.delete_remote_branch("a..b").await.unwrap_err();
        assert!(matches!(err, GitError::InvalidBranch { .. }), "{err:?}");
      }
    }
  };
}

pub(crate) use backend_contract_tests;

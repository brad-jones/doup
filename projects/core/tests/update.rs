//! Runs [`doup_core::Updater`] end to end against real git remotes on disk, with fake plugins, using every git
//! backend.

mod support;

use doup_core::reconcile::{CloseReason, CreateReason, DeleteReason, SkipReason, UpdateReason};
use doup_core::{Action, ActionOutcome, RepoError, RepoReport, UpdateReport, Updater};
use doup_git::UpdateBranch;
use std::sync::atomic::Ordering;
use std::time::Duration;
use support::*;

fn branch(name: &str, latest: bool) -> String {
  UpdateBranch::new(&plugin(ECOSYSTEM), &dependency(name, "0.0.0"), latest).to_string()
}

fn summary(repo: &RepoReport) -> Vec<String> {
  repo.actions().iter().map(|report| format!("{} => {:?}", report.action(), report.outcome())).collect()
}

fn only_repo(report: &UpdateReport) -> &RepoReport {
  assert_eq!(report.repos().len(), 1);
  &report.repos()[0]
}

fn assert_success(report: &UpdateReport) {
  for repo in report.repos() {
    assert!(
      repo.is_success(),
      "{}: {:?} {:?} {:#?}",
      repo.origin(),
      repo.error(),
      repo.dependency_errors(),
      summary(repo)
    );
  }
}

/// Every test, run with `$backend`.
macro_rules! update_tests {
  ($backend:expr) => {
    use super::*;

    fn updater(repos: Vec<doup_config::bot::DoupBotRepo>, plugins: &FakePlugins) -> Updater<FakePlugins> {
      support::updater(doup_git::Git::new($backend), repos, plugins)
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn opens_a_pull_request_per_update_then_leaves_them_be() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", Some("2.0.0")));
      *ecosystem.notes.lock().unwrap() = vec![note("1.1.0"), note("2.0.0")];

      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      let actions = only_repo(&report).actions();
      assert!(actions.iter().all(|a| matches!(a.action(), Action::CreatePr { reason: CreateReason::NoExistingPr, .. })));
      assert!(actions.iter().all(|a| matches!(a.outcome(), ActionOutcome::Done { url: Some(_) })));

      let next = platform.open_pr(&branch("serde", false));
      assert_eq!(next.title, "chore(doup): update serde from 1.0.0 to 1.1.0");
      assert_eq!((next.version.as_str(), next.base_branch.as_str()), ("1.1.0", "main"));
      assert!(next.body.contains("<summary>1.1.0</summary>") && !next.body.contains("<summary>2.0.0</summary>"));
      let latest = platform.open_pr(&branch("serde", true));
      assert!(latest.body.contains("[!WARNING]") && latest.body.contains("<summary>2.0.0</summary>"));
      // Fetched once, up to the newest version, for both pull requests.
      assert_eq!(*ecosystem.fetched_notes.lock().unwrap(), [("serde".to_string(), "2.0.0".to_string())]);

      assert_eq!(remote.file(&branch("serde", false), "serde.version").as_deref(), Some("1.1.0"));
      assert_eq!(remote.file(&branch("serde", true), "serde.version").as_deref(), Some("2.0.0"));
      assert!(remote.contains(&branch("serde", false), "main"));

      let tips = (remote.tip(&branch("serde", false)), remote.tip(&branch("serde", true)));
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      let actions = only_repo(&report).actions();
      assert!(
        actions.iter().all(|a| matches!(a.action(), Action::Skip { reason: SkipReason::AlreadyProposed { .. }, .. })),
        "{:#?}",
        summary(only_repo(&report))
      );
      assert_eq!(platform.prs().len(), 2);
      assert_eq!((remote.tip(&branch("serde", false)), remote.tip(&branch("serde", true))), tips);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_dry_run_plans_every_repo_but_changes_nothing() {
      let (first, second) = (Remote::new(), Remote::new());
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      ecosystem.set("tokio", "1.0.0", versions("1.2.0", None));

      let report = updater(vec![first.bot_repo(), second.bot_repo()], &plugins).dry_run(true).run().await;
      assert_success(&report);
      assert_eq!(report.repos().len(), 2, "every repo is planned, not just the first");
      for repo in report.repos() {
        assert_eq!(repo.actions().len(), 2, "{:#?}", summary(repo));
        assert!(repo.actions().iter().all(|a| matches!(a.outcome(), ActionOutcome::Planned)));
      }
      assert!(platform.prs().is_empty());
      assert_eq!(first.branches(), ["main"]);
      assert_eq!(second.branches(), ["main"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failing_repo_does_not_stop_the_others() {
      let good = Remote::new();
      let bad_plugin = Remote::with_plugins("./missing.wasm", ECOSYSTEM);
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      let missing = doup_config::bot::DoupBotRepo { origin: "file:///does/not/exist.git".into(), auth: None };

      let repos = vec![bad_plugin.bot_repo(), missing, good.bot_repo()];
      let report = updater(repos, &plugins).run().await;

      let [bad_plugin_report, missing_report, good_report] = report.repos() else { panic!("{report:#?}") };
      assert!(matches!(bad_plugin_report.error(), Some(RepoError::LoadPlatform(_))), "{bad_plugin_report:?}");
      assert!(matches!(missing_report.error(), Some(RepoError::Clone(_))), "{missing_report:?}");
      assert!(good_report.is_success(), "{good_report:?}");
      assert_eq!(report.failed_repos().count(), 2);
      assert_eq!(platform.open_prs().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_failed_lookup_leaves_the_dependencys_pull_requests_alone() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      ecosystem.set("tokio", "1.0.0", versions("1.2.0", None));
      updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_eq!(platform.open_prs().len(), 2);

      ecosystem.fail_lookup("serde");
      ecosystem.set("tokio", "1.0.0", versions("1.3.0", None));
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      let repo = only_repo(&report);
      assert!(!repo.is_success());
      assert_eq!(repo.dependency_errors().len(), 1);
      assert!(repo.dependency_errors()[0].to_string().contains("'serde'"));
      // The other dependency is still updated.
      assert!(
        matches!(&repo.actions()[..], [a] if matches!(a.action(), Action::UpdatePr { reason: UpdateReason::NewerVersion { .. }, .. })),
        "{:#?}",
        summary(repo)
      );
      assert_eq!(platform.open_prs().len(), 2);
      assert!(remote.tip(&branch("serde", false)).is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn closes_pull_requests_that_are_no_longer_needed_and_deletes_their_branches() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      ecosystem.set("tokio", "1.0.0", versions("1.2.0", None));
      updater(vec![remote.bot_repo()], &plugins).run().await;

      ecosystem.set("serde", "1.1.0", None);
      ecosystem.remove("tokio");
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      let summary = summary(only_repo(&report));
      assert_eq!(summary.len(), 4, "{summary:#?}");

      let serde = platform.pr(1);
      assert!(!serde.open);
      assert!(serde.comments[0].starts_with(&CloseReason::UpToDate.to_string()), "{:?}", serde.comments);
      let tokio = platform.pr(2);
      assert!(tokio.comments[0].starts_with(&CloseReason::DependencyNotFound.to_string()), "{:?}", tokio.comments);
      assert_eq!(remote.branches(), ["main"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_branch_is_kept_when_closing_its_pull_request_fails() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      updater(vec![remote.bot_repo()], &plugins).run().await;

      ecosystem.set("serde", "1.1.0", None);
      platform.fail_close.lock().unwrap().insert(1);
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      let repo = only_repo(&report);
      assert!(
        matches!(&repo.actions()[..], [close, delete]
          if matches!(close.outcome(), ActionOutcome::Failed(_)) && matches!(delete.outcome(), ActionOutcome::NotAttempted)),
        "{:#?}",
        summary(repo)
      );
      assert!(!report.is_success());
      assert!(platform.pr(1).open);
      assert!(remote.tip(&branch("serde", false)).is_some());
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn rebuilds_a_pull_request_that_has_fallen_behind() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      updater(vec![remote.bot_repo()], &plugins).run().await;

      remote.commit("main", "dev@example.com", "README", "moved on");
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      assert!(
        matches!(&only_repo(&report).actions()[..], [a] if matches!(a.action(), Action::UpdatePr { reason: UpdateReason::BehindDefaultBranch, .. })),
        "{:#?}",
        summary(only_repo(&report))
      );
      assert!(remote.contains(&branch("serde", false), "main"));
      assert!(platform.pr(1).body.contains(&UpdateReason::BehindDefaultBranch.to_string()));
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn never_rebuilds_a_branch_someone_else_has_pushed_to() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      updater(vec![remote.bot_repo()], &plugins).run().await;

      let serde = branch("serde", false);
      remote.commit(&serde, "someone@example.com", "fix.txt", "a fix");
      let tip = remote.tip(&serde);
      ecosystem.set("serde", "1.0.0", versions("1.2.0", None));
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      assert!(
        matches!(&only_repo(&report).actions()[..], [a] if matches!(a.action(), Action::Skip { reason: SkipReason::ForeignCommits { .. }, .. })),
        "{:#?}",
        summary(only_repo(&report))
      );
      assert_eq!(remote.tip(&serde), tip);
      assert_eq!(platform.pr(1).version, "1.1.0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn a_closed_pull_request_ignores_its_version_and_its_branch_is_cleaned_up() {
      let remote = Remote::new();
      let (plugins, platform, ecosystem) = FakePlugins::single();
      ecosystem.set("serde", "1.0.0", versions("1.1.0", None));
      updater(vec![remote.bot_repo()], &plugins).run().await;
      platform.prs.lock().unwrap()[0].open = false;

      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      assert!(
        matches!(&only_repo(&report).actions()[..], [skip, delete]
          if matches!(skip.action(), Action::Skip { reason: SkipReason::PreviouslyClosed { .. }, .. })
          && matches!(delete.action(), Action::DeleteBranch { reason: DeleteReason::PrClosed { .. }, .. })),
        "{:#?}",
        summary(only_repo(&report))
      );
      assert_eq!(remote.branches(), ["main"]);

      // A newer version gets a new pull request, on the same branch name.
      ecosystem.set("serde", "1.0.0", versions("1.2.0", None));
      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      assert!(matches!(
        only_repo(&report).actions()[0].action(),
        Action::CreatePr { reason: CreateReason::PreviousPrClosed { .. }, .. }
      ));
      assert_eq!(platform.open_pr(&branch("serde", false)).version, "1.2.0");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn deletes_a_merged_doup_branch_without_commits_of_its_own() {
      let remote = Remote::new();
      let (plugins, _platform, _ecosystem) = FakePlugins::single();
      let merged = "doup/eco/next/merged-1234";
      remote.branch_at(merged, "main");
      remote.commit("main", "dev@example.com", "README", "moved on");

      let report = updater(vec![remote.bot_repo()], &plugins).run().await;
      assert_success(&report);
      assert!(
        matches!(&only_repo(&report).actions()[..], [a] if matches!(a.action(), Action::DeleteBranch { reason: DeleteReason::Orphaned, .. })),
        "{:#?}",
        summary(only_repo(&report))
      );
      assert_eq!(remote.branches(), ["main"]);
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn applying_updates_is_limited_across_every_repo() {
      let (first, second) = (Remote::new(), Remote::with_plugins("./other-platform.wasm", ECOSYSTEM));
      let (mut plugins, platform, ecosystem) = FakePlugins::single();
      let other_platform = std::sync::Arc::new(FakePlatform::default());
      plugins.platforms.insert("./other-platform.wasm".to_string(), other_platform.clone());
      *ecosystem.apply_delay.lock().unwrap() = Duration::from_millis(100);
      for name in ["a", "b", "c"] {
        ecosystem.set(name, "1.0.0", versions("1.1.0", None));
      }

      let report =
        updater(vec![first.bot_repo(), second.bot_repo()], &plugins).concurrency(concurrency(2, 4, 2)).run().await;
      assert_success(&report);
      assert_eq!((platform.open_prs().len(), other_platform.open_prs().len()), (3, 3));
      assert_eq!(ecosystem.max_concurrent_applies.load(Ordering::SeqCst), 2);
    }
  };
}

mod git_cli {
  update_tests!(doup_git::GitCli);
}

mod grit {
  update_tests!(doup_git::Grit);
}

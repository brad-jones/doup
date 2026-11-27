//! The actions [`super::reconcile`] plans.

use super::{CloseReason, CreateReason, DeleteReason, SkipReason, UpdateReason};
use doup_config::repo::DoupRepoPlugin;
use doup_git::UpdateBranch;
use doup_plugin_host::ecosystem_plugin::Dependency;
use std::fmt;

/// The number of a pull request, rendered as `#<number>`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PrNumber(i64);

impl PrNumber {
  pub fn new(number: i64) -> Self {
    Self(number)
  }

  pub fn get(self) -> i64 {
    self.0
  }
}

impl From<i64> for PrNumber {
  fn from(number: i64) -> Self {
    Self(number)
  }
}

impl fmt::Display for PrNumber {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(f, "#{}", self.0)
  }
}

/// Which of a dependency's available versions an update targets. Each kind gets a branch, & so a pull
/// request, of its own.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TargetKind {
  /// The newest version within the dependency's version constraints, ie: `versions.next`.
  Next,

  /// The newest version regardless of the dependency's version constraints, ie: `versions.latest`.
  /// It may well contain breaking changes.
  Latest,
}

/// A version an update targets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
  pub version: String,
  pub kind: TargetKind,
}

impl Target {
  pub fn new(version: impl Into<String>, kind: TargetKind) -> Self {
    Self { version: version.into(), kind }
  }

  pub fn is_latest(&self) -> bool {
    self.kind == TargetKind::Latest
  }
}

impl fmt::Display for Target {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self.kind {
      TargetKind::Next => f.write_str(&self.version),
      TargetKind::Latest => write!(f, "{} (latest)", self.version),
    }
  }
}

/// An update of one dependency to one version.
#[derive(Debug, Clone)]
pub struct PlannedUpdate {
  /// The ecosystem plugin that discovered the dependency, & so applies the update.
  pub ecosystem: DoupRepoPlugin,

  pub dependency: Dependency,

  pub target: Target,

  /// The dependency's other update, if it has one, which has a pull request of its own.
  pub companion: Option<Target>,
}

impl PlannedUpdate {
  /// The branch this update is pushed to, see [`UpdateBranch`].
  pub fn branch(&self) -> String {
    UpdateBranch::new(&self.ecosystem, &self.dependency, self.target.is_latest()).to_string()
  }

  /// The dependency's name, prefixed by its registry when it is not from the ecosystem's default registry.
  pub fn dependency_label(&self) -> String {
    match &self.dependency.registry {
      Some(registry) => format!("{registry}/{}", self.dependency.name),
      None => self.dependency.name.clone(),
    }
  }
}

impl fmt::Display for PlannedUpdate {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    write!(
      f,
      "{} {} {} -> {}",
      self.ecosystem.name(),
      self.dependency_label(),
      self.dependency.current_version,
      self.target
    )
  }
}

/// Something doup should do, & why.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum Action {
  /// Push the update to its branch & open a pull request for it.
  CreatePr { update: PlannedUpdate, reason: CreateReason },

  /// Rebuild the update on its branch & refresh the open pull request `number`.
  UpdatePr { number: PrNumber, update: PlannedUpdate, reason: UpdateReason },

  /// Close the open pull request `number`, posting `reason` as a comment.
  ClosePr { number: PrNumber, head_branch: String, reason: CloseReason },

  /// Delete `branch` from `origin`.
  DeleteBranch { branch: String, reason: DeleteReason },

  /// Do nothing about this update.
  Skip { update: PlannedUpdate, reason: SkipReason },
}

impl Action {
  /// Why doup decided on this action, as sentences for the people who maintain the repo.
  pub fn reason(&self) -> &dyn fmt::Display {
    match self {
      Self::CreatePr { reason, .. } => reason,
      Self::UpdatePr { reason, .. } => reason,
      Self::ClosePr { reason, .. } => reason,
      Self::DeleteBranch { reason, .. } => reason,
      Self::Skip { reason, .. } => reason,
    }
  }

  /// The update this action is about, if any.
  pub fn update(&self) -> Option<&PlannedUpdate> {
    match self {
      Self::CreatePr { update, .. } | Self::UpdatePr { update, .. } | Self::Skip { update, .. } => Some(update),
      Self::ClosePr { .. } | Self::DeleteBranch { .. } => None,
    }
  }

  /// The update this action pushes to its branch, if any.
  pub(crate) fn pushes_update(&self) -> Option<&PlannedUpdate> {
    match self {
      Self::CreatePr { update, .. } | Self::UpdatePr { update, .. } => Some(update),
      _ => None,
    }
  }
}

/// A one line summary, eg: `create PR: cargo serde 1.0.0 -> 1.1.0`. See [`Action::reason`] for the why.
impl fmt::Display for Action {
  fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
    match self {
      Self::CreatePr { update, .. } => write!(f, "create PR: {update}"),
      Self::UpdatePr { number, update, .. } => write!(f, "update PR {number}: {update}"),
      Self::ClosePr { number, head_branch, .. } => write!(f, "close PR {number} ({head_branch})"),
      Self::DeleteBranch { branch, .. } => write!(f, "delete branch {branch}"),
      Self::Skip { update, .. } => write!(f, "skip: {update}"),
    }
  }
}

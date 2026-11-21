use anyhow::Result;
use galvanize_rs::{GList, GPath, GString, class, class_interface, class_methods};
use std::env::current_dir;

/// The result of [`IConfigFile::load`].
pub struct Loaded<T> {
  /// Exactly what is in the file, env overrides excluded. Keeps its source so `save` works.
  /// `None` when no config file was found.
  pub file: Option<T>,

  /// The file with env overrides applied. Has no source, so it can never be saved.
  pub merged: T,
}

#[class_interface]
pub trait IConfigFile: Sized {
  /// The name(s) of the config file(s) that locate_file will search for.
  const DEFAULT_NAMES: &'static [&'static str];

  /// Recurses up from the given start_dir (defaulting to the current working directory)
  /// looking for the standard doup config file names, returning the first match otherwise None.
  fn locate_file(start_dir: Option<GPath>) -> Result<Option<GPath>>;
}

#[class(abstract, implements = IConfigFile)]
pub struct ConfigFile {}

#[class_methods]
impl ConfigFile {
  #[class_abstract]
  pub const DEFAULT_NAMES: &'static [&'static str];

  /// Recurses up from the given start_dir (defaulting to the current working directory)
  /// looking for the standard doup config file names, returning the first match otherwise None.
  #[class_virtual]
  pub fn locate_file(start_dir: Option<GPath>) -> Result<Option<GPath>> {
    let dir = match start_dir {
      Some(d) => d,
      None => current_dir()?.into(),
    };

    Ok(
      dir
        .ancestors()
        .flat_map(|dir| Self::DEFAULT_NAMES.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file()),
    )
  }
}

#[class(extends = ConfigFile)]
pub struct DoupBotConfigFile {}

#[class_methods]
impl DoupBotConfigFile {
  pub const DEFAULT_NAMES: &'static [&'static str] = &["doupbot.json", "doupbot.yaml", "doupbot.yml"];
}

#[class(extends = ConfigFile)]
pub struct DoupRepoConfigFile {}

#[class_methods]
impl DoupRepoConfigFile {
  pub const DEFAULT_NAMES: &'static [&'static str] = &["doup.json", "doup.yaml", "doup.yml"];
}

#[cfg(test)]
mod tests {
  use super::*;
  use assert_fs::prelude::*;

  fn locate<T: IConfigFile>(dir: &std::path::Path) -> Option<GPath> {
    T::locate_file(Some(dir.into())).unwrap()
  }

  #[test]
  fn locate_file_searches_for_each_classes_default_names() {
    let root = assert_fs::TempDir::new().unwrap();
    root.child("doupbot.yaml").touch().unwrap();
    let nested = root.child("a/b");
    nested.create_dir_all().unwrap();

    let found = DoupBotConfigFile::locate_file(Some(nested.path().into())).unwrap();
    assert_eq!(found, Some(root.child("doupbot.yaml").path().into()));
    assert_eq!(
      DoupRepoConfigFile::locate_file(Some(nested.path().into())).unwrap(),
      None
    );
    assert_eq!(DoupRepoConfigFile::DEFAULT_NAMES[0], "doup.json");

    assert!(locate::<DoupBotConfigFile>(nested.path()).is_some());
    assert!(locate::<DoupRepoConfigFile>(nested.path()).is_none());
  }
}

use anyhow::Result;
use galvanize_rs::{class, class_methods};
use std::path::{Path, PathBuf};

pub trait IConfigFile {
  /// This is the default doc comment if no other method gives one
  fn locate_file(&self, start_dir: Option<&Path>) -> Result<Option<PathBuf>>;
}

#[class(abstract, implements = IConfigFile)]
pub struct ConfigFile {}

#[class_methods]
impl ConfigFile {
  /// This doc comment overrides the trait comment
  #[class_abstract]
  fn locate_file(&self, start_dir: Option<&Path>) -> Result<Option<PathBuf>>;
}

#[class(extends = ConfigFile)]
pub struct DoupBotConfigFile {}

#[class_methods]
impl DoupBotConfigFile {
  /// This doc comment would overrides the abstract comment
  fn locate_file(&self, start_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    todo!()
  }
}

#[class(extends = ConfigFile)]
pub struct DoupRepoConfigFile {}

#[class_methods]
impl DoupRepoConfigFile {
  fn locate_file(&self, start_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    todo!()
  }
}

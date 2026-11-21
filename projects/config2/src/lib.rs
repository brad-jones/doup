use anyhow::Result;
use galvanize_rs::{GList, GPath, GString, class, class_interface, class_methods};
use std::env::current_dir;

#[class_interface]
pub trait IConfigFile {
  /// This is the default doc comment if no other method gives one
  fn locate_file(&self, start_dir: Option<GPath>) -> Result<Option<GPath>>;
}

#[class(abstract, implements = IConfigFile)]
pub struct ConfigFile {}

#[class_methods]
impl ConfigFile {
  /// The name(s) of the config file(s) that locate_file will search for.
  #[class_abstract]
  fn default_names(&self) -> GList<GString>;

  /// Recurses up from the given start_dir (defaulting to the current working directory)
  /// looking for the standard doup config file names, returning the first match otherwise None.
  fn locate_file(&self, start_dir: Option<GPath>) -> Result<Option<GPath>> {
    let dir = match start_dir {
      Some(d) => d,
      None => current_dir()?.into(),
    };

    Ok(
      dir
        .ancestors()
        .flat_map(|dir| self.default_names().iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file()),
    )
  }
}

/*
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
*/

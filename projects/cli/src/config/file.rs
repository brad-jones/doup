use super::env;
use anyhow::Result;
use jsonschema::ValidationError;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::Value;
use std::env::current_dir;
use std::fs::{read_to_string, write};
use std::path::{Path, PathBuf};

/// The result of [`ConfigFile::load`].
#[derive(Debug)]
pub struct Loaded<T> {
  /// Exactly what is in the file, env overrides excluded. Keeps its source so `save` works.
  /// `None` when no config file was found.
  pub file: Option<T>,

  /// The file with env overrides applied. Has no source, so it can never be saved.
  pub merged: T,
}

/// [`ConfigFile::load`] with the env supplied by the caller rather than read from the process.
/// The env is merged over the file before validating, so it may be a partial override.
/// A free function (not a trait method) so it stays out of the trait's public API; tests use it.
pub(super) fn load_with_env<T: ConfigFile>(
  start_dir: Option<&Path>,
  env: Value,
) -> Result<Loaded<T>> {
  let file = match T::locate_file(start_dir)? {
    Some(path) => Some(T::from_file(&path)?),
    None => None,
  };

  let mut merged = match &file {
    Some(file) => serde_json::to_value(file)?,
    None => Value::Object(Default::default()),
  };
  env::merge(&mut merged, env);
  // `source` is `#[serde(skip)]` so the merged instance comes back without one.
  let merged = T::validated(merged)?;

  Ok(Loaded { file, merged })
}

pub trait ConfigFile: Serialize + DeserializeOwned + Sized {
  /// The `validate` function of the type's embedded `#[jsonschema::validator]`.
  const VALIDATE: for<'i> fn(&'i Value) -> Result<(), ValidationError<'i>>;

  /// The name(s) of the config file(s) that locate_file will search for.
  const DEFAULT_NAMES: &'static [&'static str];

  /// Env vars named `<ENV_PREFIX>_<PATH>` are read by `from_env` and `load`, eg: `DOUP_FOO_0_BAR=x`.
  /// Must be non-empty and unique among configs (`<ENV_PREFIX>_` must not prefix another's).
  const ENV_PREFIX: &'static str;

  // Used by `save` to save a loaded file back to the same place it came from.
  fn source(&self) -> Option<&Path>;

  /// Used by `from_file`` to store the file path into memory so that
  /// after loading the file, it can be saved back to the same file.
  fn set_source(&mut self, path: PathBuf);

  /// Runs after every successful schema validation & deserialization,
  /// for work the schema can't express (eg: reading a referenced file into memory).
  fn hydrate(&mut self) -> Result<()> {
    Ok(())
  }

  /// Recurses up from the given start_dir (defaulting to the current working directory)
  /// looking for the standard doup config file names, returning the first match otherwise None.
  fn locate_file(start_dir: Option<&Path>) -> Result<Option<PathBuf>> {
    let dir = match start_dir {
      Some(d) => d.to_path_buf(),
      None => current_dir()?,
    };

    Ok(
      dir
        .ancestors()
        .flat_map(|dir| Self::DEFAULT_NAMES.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file()),
    )
  }

  /// Convenience function for the default case of loading a config file, with env overrides.
  /// Conceptually the same as: `ConfigFile::from_file(ConfigFile::locate_file()) + ConfigFile::from_env()`.
  /// `start_dir` sets the initial search dir used by `locate_file`, otherwise the current working directory is used.
  ///
  /// If no config file is found, the merged result is built from the env alone.
  ///
  /// The merged result must still satisfy the schema as a whole, so an override that produces an
  /// invalid combination (eg: adding a `pass` to a repo whose file entry uses a `key`) is an error.
  ///
  /// Most of the time if you are just reading config, you will want the `merged` instance.
  /// If you want to persist any changes to the config, use the returned `file` instance.
  fn load(start_dir: Option<&Path>) -> Result<Loaded<Self>> {
    load_with_env(start_dir, env::env_value(Self::ENV_PREFIX))
  }

  /// Saves the config back to the file it was loaded from.
  fn save(&self) -> Result<()> {
    let Some(path) = self.source() else {
      anyhow::bail!("config has no source path; use to_file instead");
    };
    self.to_file(path)
  }

  /// Hydrates a new instance of the config file from a given file path, either JSON or YAML.
  fn from_file(path: &Path) -> Result<Self> {
    let data = read_to_string(path)?;
    let mut config = match path.extension().and_then(|e| e.to_str()) {
      Some("json") => Self::from_json(&data)?,
      Some("yaml" | "yml") => Self::from_yaml(&data)?,
      _ => anyhow::bail!("unsupported config file extension: {}", path.display()),
    };
    config.set_source(path.to_path_buf());
    Ok(config)
  }

  /// Saves the current instance of the config file to the given file path.
  /// Selecting the final representation (JSON or YAML) based on the file extension.
  fn to_file(&self, path: &Path) -> Result<()> {
    Ok(write(
      path,
      match path.extension().and_then(|e| e.to_str()) {
        Some("json") => self.to_json()?,
        Some("yaml" | "yml") => self.to_yaml()?,
        _ => anyhow::bail!("unsupported config file extension: {}", path.display()),
      },
    )?)
  }

  /// Takes a parsed json value, validates it against the the json schema,
  /// returning the final type if valid or the json schema validation errors.
  fn validated(instance: Value) -> Result<Self> {
    (Self::VALIDATE)(&instance).map_err(|e| e.to_owned())?;
    let mut config: Self = serde_json::from_value(instance)?;
    config.hydrate()?;
    Ok(config)
  }

  /// Hydrates a new instance of the config file from a JSON string.
  fn from_json(value: &str) -> Result<Self> {
    Self::validated(serde_json::from_str::<serde_json::Value>(value)?)
  }

  /// Serializes the config file into it's JSON representation.
  fn to_json(&self) -> Result<String> {
    Ok(serde_json::to_string_pretty(self)?)
  }

  /// Hydrates a new instance of the config file from a YAML string.
  fn from_yaml(value: &str) -> Result<Self> {
    Self::validated(serde_saphyr::from_str::<serde_json::Value>(value)?)
  }

  /// Serializes the config file into it's YAML representation.
  fn to_yaml(&self) -> Result<String> {
    Ok(serde_saphyr::to_string(self)?)
  }

  /// Hydrates a new instance of the config file from the process environment alone.
  /// Env values are always strings and keys are lowercased, see [`env::env_value`].
  fn from_env() -> Result<Self> {
    Self::validated(env::env_value(Self::ENV_PREFIX))
  }
}

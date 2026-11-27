//! Convert environment variables into a nested JSON value, the same way .NET's configuration reads
//! them: the prefix and a single `_`, then a path whose segments are separated by `__`.
//!
//! Examples (prefix = "APP"):
//!   APP_GIT_BACKEND=x       -> {"git_backend":"x"}
//!   APP_FOO__BAR=baz        -> {"foo":{"bar":"baz"}}
//!   APP_LIST__0=a           -> {"list":["a"]}
//!   APP_LIST__1__KEY_PATH=x -> {"list":[null,{"key_path":"x"}]}
//!
//! A single `_` is always part of a key, so keys may contain `_` at any depth.
//!
//! Limitations:
//! - Values are always strings; schemas accept numeric strings wherever they take a number.
//! - Names are lowercased, so fields containing capitals can't be targeted.
//! - Every var sharing the prefix becomes a key, and the schemas reject keys they don't know
//!   (other than in plugin `with` config), so eg: `DOUP_BOT_LOG` fails validation.
//! - A segment that is empty, or starts or ends with `_`, is an error rather than being guessed at,
//!   eg: `APP__X` (the old form of `APP_X`), `APP_X___Y` or `APP_X__`.

use super::ConfigError;
use serde_json::{Map, Value};

/// Numeric segments above this are treated as plain object keys, so a stray
/// `FOO_999999999=x` can't make us allocate a gigantic array.
const MAX_ARRAY_INDEX: usize = 65_535;

/// The separator between path segments.
const SEPARATOR: &str = "__";

#[derive(Debug, Clone, PartialEq)]
enum Seg {
  Key(String),
  Index(usize),
}

/// Reads the process environment, see [`layered_vars_to_value`].
/// Vars that are not valid unicode are skipped.
pub fn env_value(prefix: &str, scope: Option<&str>) -> Result<Value, ConfigError> {
  layered_vars_to_value(
    std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    prefix,
    scope,
  )
}

/// Builds a value from vars starting with `<prefix>_`, then, if given, merges vars starting
/// with `<scope>_` over the top, eg: prefix `DOUP_REPO` with scope `DOUP_REPO_0`.
///
/// Vars that look like a scope (`<prefix>_<n>_...`) are left out of the prefix layer, as no
/// config's top-level key is a number; this frees `<prefix>_<n>` up for use as a scope.
pub fn layered_vars_to_value<I>(vars: I, prefix: &str, scope: Option<&str>) -> Result<Value, ConfigError>
where
  I: IntoIterator<Item = (String, String)>,
{
  let vars: Vec<_> = vars.into_iter().collect();
  let needle = needle(prefix);
  let base = vars.iter().filter(|(k, _)| !k.strip_prefix(&needle).is_some_and(is_scoped));
  let mut value = vars_to_value(base.cloned(), prefix)?;
  if let Some(scope) = scope {
    merge(&mut value, vars_to_value(vars, scope)?);
  }
  Ok(value)
}

fn needle(prefix: &str) -> String {
  format!("{}_", prefix.trim_end_matches('_'))
}

/// Whether `rest` (what follows `<prefix>_`) is `<n>` or starts `<n>_`, eg: `0_PLATFORM__USE`.
fn is_scoped(rest: &str) -> bool {
  let first = rest.split('_').next().unwrap_or_default();
  !first.is_empty() && first.bytes().all(|b| b.is_ascii_digit())
}

/// Same as [`env_value`] but takes any iterator of (name, value) pairs, and has no scope.
pub fn vars_to_value<I>(vars: I, prefix: &str) -> Result<Value, ConfigError>
where
  I: IntoIterator<Item = (String, String)>,
{
  let needle = needle(prefix);

  // Sort so output is deterministic (env order is arbitrary).
  let mut matched: Vec<(String, String)> = vars.into_iter().filter(|(k, _)| k.starts_with(&needle)).collect();
  matched.sort_by(|a, b| a.0.cmp(&b.0));

  let mut root = Value::Null;

  for (name, value) in matched {
    let Some(path) = parse_path(&name[needle.len()..]) else {
      return Err(ConfigError::InvalidEnvVar { name });
    };
    insert(&mut root, &path, Value::String(value));
  }

  if root.is_null() {
    root = Value::Object(Map::new());
  }
  Ok(root)
}

/// Overlays `over` onto `base`. Objects merge by key and arrays by index;
/// a `Null` in `over` means "no override" (sparse array padding), anything else replaces.
pub fn merge(base: &mut Value, over: Value) {
  match (base, over) {
    (_, Value::Null) => {}
    (Value::Object(base), Value::Object(over)) => {
      for (key, value) in over {
        match base.get_mut(&key) {
          Some(slot) => merge(slot, value),
          None => {
            base.insert(key, value);
          }
        }
      }
    }
    (Value::Array(base), Value::Array(over)) => {
      for (i, value) in over.into_iter().enumerate() {
        match base.get_mut(i) {
          Some(slot) => merge(slot, value),
          None => base.push(value),
        }
      }
    }
    (base, over) => *base = over,
  }
}

/// Splits `s` (what follows `<prefix>_`) into segments, or `None` if any is invalid.
fn parse_path(s: &str) -> Option<Vec<Seg>> {
  s.split(SEPARATOR)
    .map(|p| {
      if p.is_empty() || p.starts_with('_') || p.ends_with('_') {
        return None;
      }
      Some(match p.parse::<usize>() {
        Ok(i) if i <= MAX_ARRAY_INDEX && p.bytes().all(|b| b.is_ascii_digit()) => Seg::Index(i),
        _ => Seg::Key(p.to_lowercase()),
      })
    })
    .collect()
}

/// Recursively walks `path`, creating objects/arrays as needed.
/// If the existing node has the wrong shape (e.g. a string where an object
/// is needed), it is replaced; since vars are processed in sorted order,
/// the result is deterministic.
fn insert(node: &mut Value, path: &[Seg], leaf: Value) {
  let Some((seg, rest)) = path.split_first() else {
    *node = leaf;
    return;
  };

  match seg {
    Seg::Key(k) => {
      if !node.is_object() {
        *node = Value::Object(Map::new());
      }
      let child = node.as_object_mut().unwrap().entry(k.clone()).or_insert(Value::Null);
      insert(child, rest, leaf);
    }
    Seg::Index(i) => {
      if !node.is_array() {
        *node = Value::Array(Vec::new());
      }
      let arr = node.as_array_mut().unwrap();
      if arr.len() <= *i {
        arr.resize(*i + 1, Value::Null);
      }
      insert(&mut arr[*i], rest, leaf);
    }
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use serde_json::json;

  fn pairs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
  }

  fn run(vars: &[(&str, &str)], prefix: &str) -> Value {
    vars_to_value(pairs(vars), prefix).unwrap()
  }

  #[test]
  fn nested_objects() {
    let v = run(&[("APP_FOO__BAR", "baz")], "APP");
    assert_eq!(v, json!({"foo": {"bar": "baz"}}));
  }

  #[test]
  fn single_underscores_are_part_of_the_key_at_any_depth() {
    let v = run(&[("APP_GIT_BACKEND", "grit"), ("APP_REPOS__0__AUTH__KEY_PATH", "/k")], "APP");
    assert_eq!(v, json!({"git_backend": "grit", "repos": [{"auth": {"key_path": "/k"}}]}));
  }

  #[test]
  fn arrays_and_deep_nesting() {
    let v = run(
      &[
        ("APP_LIST__0", "a"),
        ("APP_LIST__1", "b"),
        ("APP_A__B__C__D__E", "deep"),
        ("APP_USERS__0__NAME", "x"),
        ("APP_USERS__0__TAGS__0", "t"),
      ],
      "APP",
    );
    assert_eq!(
      v,
      json!({
          "list": ["a", "b"],
          "a": {"b": {"c": {"d": {"e": "deep"}}}},
          "users": [{"name": "x", "tags": ["t"]}]
      })
    );
  }

  #[test]
  fn prefix_filters() {
    let v = run(&[("APP_X", "1"), ("OTHER_Y", "2"), ("APPLE_Z", "3")], "APP");
    assert_eq!(v, json!({"x": "1"}));
  }

  #[test]
  fn prefix_does_not_match_longer_prefix() {
    let v = run(&[("DOUPBOT_X", "1"), ("DOUP_Y", "2")], "DOUP");
    assert_eq!(v, json!({"y": "2"}));
  }

  #[test]
  fn invalid_segments_are_rejected() {
    for name in ["APP__GIT_BACKEND", "APP_X___Y", "APP_X____Y", "APP_X__", "APP_X_", "APP_"] {
      let err = vars_to_value(pairs(&[(name, "v")]), "APP").unwrap_err();
      assert!(matches!(err, ConfigError::InvalidEnvVar { name: ref n } if n == name), "{name}: {err:?}");
    }
  }

  #[test]
  fn top_level_array() {
    let v = run(&[("FOO_0", "bar")], "FOO");
    assert_eq!(v, json!(["bar"]));
  }

  #[test]
  fn indexes_above_the_max_are_keys() {
    let v = run(&[("APP_HUGE__65536", "x"), ("APP_MAX__65535", "y")], "APP");
    assert_eq!(v["huge"], json!({"65536": "x"}));
    assert_eq!(v["max"].as_array().map(Vec::len), Some(MAX_ARRAY_INDEX + 1));
  }

  #[test]
  fn signed_numbers_are_keys() {
    let v = run(&[("APP_LIST__+1", "x")], "APP");
    assert_eq!(v, json!({"list": {"+1": "x"}}));
  }

  #[test]
  fn conflicting_paths_resolve_the_same_whatever_the_env_order() {
    let vars = [("APP_A", "x"), ("APP_A__B", "y"), ("APP_L__0", "z"), ("APP_L__K", "w")];
    let expected = json!({"a": {"b": "y"}, "l": {"k": "w"}});
    assert_eq!(run(&vars, "APP"), expected);

    let mut reversed = vars;
    reversed.reverse();
    assert_eq!(run(&reversed, "APP"), expected);
  }

  #[test]
  fn no_matches_gives_empty_object() {
    assert_eq!(run(&[("A", "1")], "NOPE"), json!({}));
  }

  fn run_layered(vars: &[(&str, &str)], prefix: &str, scope: Option<&str>) -> Result<Value, ConfigError> {
    layered_vars_to_value(pairs(vars), prefix, scope)
  }

  #[test]
  fn layered_without_scope_skips_scoped_vars() {
    let v = run_layered(&[("APP_X", "1"), ("APP_0_X", "2"), ("APP_0__Y", "3"), ("APP_0", "4")], "APP", None);
    assert_eq!(v.unwrap(), json!({"x": "1"}));
  }

  #[test]
  fn layered_with_only_scoped_vars_is_an_object() {
    let v = run_layered(&[("APP_0_X", "1")], "APP", Some("APP_1"));
    assert_eq!(v.unwrap(), json!({}));
  }

  #[test]
  fn layered_scope_wins_over_prefix() {
    let v = run_layered(&[("APP_X", "1"), ("APP_Y", "y"), ("APP_0_X", "2")], "APP", Some("APP_0"));
    assert_eq!(v.unwrap(), json!({"x": "2", "y": "y"}));
  }

  #[test]
  fn layered_scope_ignores_other_scopes() {
    let v = run_layered(&[("APP_X", "1"), ("APP_10_X", "2")], "APP", Some("APP_1"));
    assert_eq!(v.unwrap(), json!({"x": "1"}));
  }

  #[test]
  fn layered_scope_supports_nesting() {
    let v = run_layered(&[("APP_0_LIST__0__SOME_KEY", "v")], "APP", Some("APP_0"));
    assert_eq!(v.unwrap(), json!({"list": [{"some_key": "v"}]}));
  }

  #[test]
  fn layered_scope_rejects_the_old_double_underscore_form() {
    let err = run_layered(&[("APP_0__LIST__0__USE", "v")], "APP", Some("APP_0")).unwrap_err();
    assert!(matches!(err, ConfigError::InvalidEnvVar { ref name } if name == "APP_0__LIST__0__USE"), "{err:?}");
  }

  #[test]
  fn merge_overlays_object_keys() {
    let mut base = json!({"a": "1", "b": {"c": "2", "d": "3"}});
    merge(&mut base, json!({"b": {"c": "9"}, "e": "4"}));
    assert_eq!(base, json!({"a": "1", "b": {"c": "9", "d": "3"}, "e": "4"}));
  }

  #[test]
  fn merge_overlays_array_by_index_and_appends() {
    let mut base = json!({"l": [{"x": "1"}, {"x": "2"}]});
    merge(&mut base, json!({"l": [null, {"x": "9"}, {"x": "3"}]}));
    assert_eq!(base, json!({"l": [{"x": "1"}, {"x": "9"}, {"x": "3"}]}));
  }

  #[test]
  fn merge_null_does_not_override() {
    let mut base = json!({"a": "1"});
    merge(&mut base, json!({"a": null}));
    assert_eq!(base, json!({"a": "1"}));
  }

  #[test]
  fn merge_replaces_on_shape_mismatch() {
    let mut base = json!({"a": {"b": "1"}});
    merge(&mut base, json!({"a": "x"}));
    assert_eq!(base, json!({"a": "x"}));
  }
}

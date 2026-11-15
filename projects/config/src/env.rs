//! Convert environment variables into a nested JSON value.
//!
//! Examples (prefix = "APP"):
//!   APP_FOO_BAR=baz   -> {"foo":{"bar":"baz"}}
//!   APP_LIST_0=a      -> {"list":["a"]}
//!   APP_LIST_1_NAME=x -> {"list":[null,{"name":"x"}]}
//!   APP_A__KEY_PATH=x -> {"a":{"key_path":"x"}}
//!
//! Names are split on `_`, unless the name contains `__` (or `__` directly follows the prefix,
//! eg: `APP__GIT_BACKEND`), then it is split on `__` only so that keys may themselves contain `_`.
//!
//! Limitations:
//! - Values are always strings; this is fine while the schemas only use strings.
//! - Names are lowercased, so fields containing capitals can't be targeted.
//! - Any var sharing the prefix (eg: `DOUP_BOT_LOG`) becomes a key. Unknown keys are ignored as
//!   neither the schemas nor serde reject them.
//! - The prefix (and scope) are matched literally, so must be written with single underscores,
//!   eg: `DOUP_REPO_0__ECOSYSTEMS__0__USE`, not `DOUP__REPO__0__ECOSYSTEMS__0__USE`.

use serde_json::{Map, Value};

/// Numeric segments above this are treated as plain object keys, so a stray
/// `FOO_999999999=x` can't make us allocate a gigantic array.
const MAX_ARRAY_INDEX: usize = 65_535;

#[derive(Debug, Clone, PartialEq)]
enum Seg {
  Key(String),
  Index(usize),
}

/// Reads the process environment, see [`layered_vars_to_value`].
/// Vars that are not valid unicode are skipped.
pub fn env_value(prefix: &str, scope: Option<&str>) -> Value {
  layered_vars_to_value(
    std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))),
    prefix,
    scope,
  )
}

/// Builds a value from vars starting with `<prefix>_`, then, if given, merges vars starting
/// with `<scope>_` over the top, eg: prefix `DOUP_REPO` with scope `DOUP_REPO_0`.
///
/// Vars whose path starts with an index (eg: `DOUP_REPO_0_X`) are left out of the prefix layer,
/// as configs are objects; this frees `<prefix>_<n>` up for use as a scope.
pub fn layered_vars_to_value<I>(vars: I, prefix: &str, scope: Option<&str>) -> Value
where
  I: IntoIterator<Item = (String, String)>,
{
  let vars: Vec<_> = vars.into_iter().collect();
  let needle = needle(prefix);
  let base = vars.iter().filter(|(k, _)| {
    !k.strip_prefix(&needle)
      .is_some_and(|rest| matches!(parse_path(rest).first(), Some(Seg::Index(_))))
  });
  let mut value = vars_to_value(base.cloned(), prefix);
  if let Some(scope) = scope {
    merge(&mut value, vars_to_value(vars, scope));
  }
  value
}

fn needle(prefix: &str) -> String {
  format!("{}_", prefix.trim_end_matches('_'))
}

/// Same as [`env_value`] but takes any iterator of (name, value) pairs.
pub fn vars_to_value<I>(vars: I, prefix: &str) -> Value
where
  I: IntoIterator<Item = (String, String)>,
{
  let needle = needle(prefix);

  // Sort so output is deterministic (env order is arbitrary).
  let mut matched: Vec<(String, String)> = vars
    .into_iter()
    .filter(|(k, _)| k.starts_with(&needle))
    .collect();
  matched.sort_by(|a, b| a.0.cmp(&b.0));

  let mut root = Value::Null;

  for (name, value) in matched {
    let path = parse_path(&name[needle.len()..]);
    if path.is_empty() {
      continue;
    }
    insert(&mut root, &path, Value::String(value));
  }

  if root.is_null() {
    root = Value::Object(Map::new());
  }
  root
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

fn parse_path(s: &str) -> Vec<Seg> {
  // `s` is what follows `<prefix>_`, so `PREFIX__KEY` arrives as `_KEY`.
  let sep = if s.contains("__") || s.starts_with('_') {
    "__"
  } else {
    "_"
  };
  s.trim_start_matches('_')
    .split(sep)
    .filter(|p| !p.is_empty())
    .map(|p| match p.parse::<usize>() {
      Ok(i) if i <= MAX_ARRAY_INDEX && p.bytes().all(|b| b.is_ascii_digit()) => Seg::Index(i),
      _ => Seg::Key(p.to_lowercase()),
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
      let child = node
        .as_object_mut()
        .unwrap()
        .entry(k.clone())
        .or_insert(Value::Null);
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

  fn run(pairs: &[(&str, &str)], prefix: &str) -> Value {
    let vars = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string()));
    vars_to_value(vars, prefix)
  }

  #[test]
  fn nested_objects() {
    let v = run(&[("APP_FOO_BAR", "baz")], "APP");
    assert_eq!(v, json!({"foo": {"bar": "baz"}}));
  }

  #[test]
  fn arrays_and_deep_nesting() {
    let v = run(
      &[
        ("APP_LIST_0", "a"),
        ("APP_LIST_1", "b"),
        ("APP_A_B_C_D_E", "deep"),
        ("APP_USERS_0_NAME", "x"),
        ("APP_USERS_0_TAGS_0", "t"),
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
  fn double_underscore_keeps_single_underscores_in_keys() {
    let v = run(
      &[
        ("APP_REPOS__0__AUTH__KEY_PATH", "/k"),
        ("APP__PLAIN__SOME_KEY", "v"),
        ("APP_OTHER_0_NAME", "x"),
      ],
      "APP",
    );
    assert_eq!(
      v,
      json!({
          "repos": [{"auth": {"key_path": "/k"}}],
          "plain": {"some_key": "v"},
          "other": [{"name": "x"}]
      })
    );
  }

  #[test]
  fn double_underscore_after_prefix_allows_a_top_level_key_with_underscores() {
    let v = run(&[("APP__GIT_BACKEND", "grit"), ("APP_PLAIN", "x")], "APP");
    assert_eq!(v, json!({"git_backend": "grit", "plain": "x"}));
  }

  #[test]
  fn top_level_array() {
    let v = run(&[("FOO_0", "bar")], "FOO");
    assert_eq!(v, json!(["bar"]));
  }

  #[test]
  fn no_matches_gives_empty_object() {
    assert_eq!(run(&[("A", "1")], "NOPE"), json!({}));
  }

  fn run_layered(pairs: &[(&str, &str)], prefix: &str, scope: Option<&str>) -> Value {
    let vars = pairs.iter().map(|(k, v)| (k.to_string(), v.to_string()));
    layered_vars_to_value(vars, prefix, scope)
  }

  #[test]
  fn layered_without_scope_skips_indexed_vars() {
    let v = run_layered(
      &[("APP_X", "1"), ("APP_0_X", "2"), ("APP__0__Y", "3")],
      "APP",
      None,
    );
    assert_eq!(v, json!({"x": "1"}));
  }

  #[test]
  fn layered_with_only_scoped_vars_is_an_object() {
    let v = run_layered(&[("APP_0_X", "1")], "APP", Some("APP_1"));
    assert_eq!(v, json!({}));
  }

  #[test]
  fn layered_scope_wins_over_prefix() {
    let v = run_layered(
      &[("APP_X", "1"), ("APP_Y", "y"), ("APP_0_X", "2")],
      "APP",
      Some("APP_0"),
    );
    assert_eq!(v, json!({"x": "2", "y": "y"}));
  }

  #[test]
  fn layered_scope_ignores_other_scopes() {
    let v = run_layered(&[("APP_X", "1"), ("APP_10_X", "2")], "APP", Some("APP_1"));
    assert_eq!(v, json!({"x": "1"}));
  }

  #[test]
  fn layered_scope_supports_double_underscore() {
    let v = run_layered(&[("APP_0__LIST__0__SOME_KEY", "v")], "APP", Some("APP_0"));
    assert_eq!(v, json!({"list": [{"some_key": "v"}]}));
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

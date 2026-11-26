//! The HTTP plumbing shared by the plugin's exports.

use crate::config;
use serde::Deserialize;
use serde_json::Value;
use wasi_fetch::Client;

const USER_AGENT: &str = concat!("doup/", env!("CARGO_PKG_VERSION"));

#[derive(Deserialize)]
struct GraphqlResponse {
  data: Option<Value>,
  #[serde(default)]
  errors: Vec<GraphqlError>,
}

#[derive(Deserialize)]
struct GraphqlError {
  message: String,
}

/// Sends `request` (a `{ query, variables }` object) to the configured GraphQL endpoint and returns its `data`.
///
/// An unsuccessful status or any GraphQL `errors` are an error. The token is never included in an error.
pub async fn graphql(request: &Value) -> Result<Value, String> {
  let config = config::get()?;
  let url = config.graphql_url();

  let response = Client::new()
    .post(&url)
    .header("User-Agent", USER_AGENT)
    .header("Accept", "application/json")
    .header("Authorization", format!("Bearer {}", config.github_token))
    .json(request)
    .send()
    .await
    .map_err(|err| format!("request to {url} failed: {err}"))?;
  let status = response.status().as_u16();
  let body = response.into_body().text().await.map_err(|err| format!("failed to read {url}: {err}"))?;

  if !(200..300).contains(&status) {
    return Err(format!("request to {url} returned {status}"));
  }
  parse_response(&body).map_err(|err| format!("request to {url} failed: {err}"))
}

fn parse_response(body: &str) -> Result<Value, String> {
  let response: GraphqlResponse =
    serde_json::from_str(body).map_err(|err| format!("invalid GraphQL response: {err}"))?;
  if !response.errors.is_empty() {
    let messages: Vec<_> = response.errors.into_iter().map(|e| e.message).collect();
    return Err(messages.join("; "));
  }
  response.data.ok_or_else(|| "GraphQL response has no data".to_string())
}

#[cfg(test)]
mod tests {
  use super::parse_response;
  use serde_json::json;

  #[test]
  fn data_is_returned() {
    assert_eq!(parse_response(r#"{"data":{"a":1}}"#).unwrap(), json!({"a": 1}));
  }

  #[test]
  fn errors_are_joined() {
    let err = parse_response(r#"{"data":null,"errors":[{"message":"one"},{"message":"two"}]}"#).err().unwrap();
    assert_eq!(err, "one; two");
  }

  #[test]
  fn missing_data_or_garbage_is_an_error() {
    assert!(parse_response("{}").is_err());
    assert!(parse_response("<html>").is_err());
  }
}

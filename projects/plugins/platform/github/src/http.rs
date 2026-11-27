//! The HTTP plumbing shared by the plugin's exports.

use crate::{config, rest};
use serde::Deserialize;
use serde_json::Value;
use wasi_fetch::{Client, RequestBuilder};

const USER_AGENT: &str = concat!("doup/", env!("CARGO_PKG_VERSION"));

/// See <https://docs.github.com/en/rest/about-the-rest-api/api-versions>.
const API_VERSION: &str = "2022-11-28";

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

/// The HTTP methods used against the REST API.
pub enum Method {
  Post,
  Patch,
}

/// Sends `request` (a `{ query, variables }` object) to the configured GraphQL endpoint and returns its `data`.
///
/// An unsuccessful status or any GraphQL `errors` are an error. The token is never included in an error.
pub async fn graphql(request: &Value) -> Result<Value, String> {
  let url = config::get()?.graphql_url();
  let (status, body) = send(Client::new().post(&url), &url, "application/json", request).await?;
  if !(200..300).contains(&status) {
    return Err(format!("request to {url} returned {status}"));
  }
  parse_response(&body).map_err(|err| format!("request to {url} failed: {err}"))
}

/// Sends `request` as JSON to the REST endpoint `path` (eg: `/repos/acme/foo/pulls`) and returns the response JSON.
///
/// An unsuccessful status is an error, described by GitHub's own error message. The token is never included in an error.
pub async fn rest(method: Method, path: &str, request: &Value) -> Result<Value, String> {
  let url = config::get()?.rest_url(path);
  let builder = match method {
    Method::Post => Client::new().post(&url),
    Method::Patch => Client::new().patch(&url),
  };
  let builder = builder.header("X-GitHub-Api-Version", API_VERSION);
  let (status, body) = send(builder, &url, "application/vnd.github+json", request).await?;
  if !(200..300).contains(&status) {
    return Err(format!("request to {url} returned {status}: {}", rest::error_message(&body)));
  }
  serde_json::from_str(&body).map_err(|err| format!("request to {url} returned invalid JSON: {err}"))
}

/// Sends `request` as JSON with the common headers, returning the status & body text.
async fn send(builder: RequestBuilder, url: &str, accept: &str, request: &Value) -> Result<(u16, String), String> {
  let config = config::get()?;
  let response = builder
    .header("User-Agent", USER_AGENT)
    .header("Accept", accept)
    .header("Authorization", format!("Bearer {}", config.github_token))
    .json(request)
    .send()
    .await
    .map_err(|err| format!("request to {url} failed: {err}"))?;
  let status = response.status().as_u16();
  let body = response.into_body().text().await.map_err(|err| format!("failed to read {url}: {err}"))?;
  Ok((status, body))
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

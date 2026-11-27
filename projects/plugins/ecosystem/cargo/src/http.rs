//! The HTTP plumbing shared by the plugin's exports.

use crate::config;
use crate::error::CargoError;
use wasi_fetch::Client;

const USER_AGENT: &str = concat!("doup/", env!("CARGO_PKG_VERSION"));

pub struct Response {
  pub status: u16,
  pub body: String,
}

impl Response {
  /// The body if the status was successful, otherwise an error naming `url`.
  pub fn into_success(self, url: &str) -> Result<String, CargoError> {
    if (200..300).contains(&self.status) {
      Ok(self.body)
    } else {
      Err(CargoError::Status { url: url.to_string(), status: self.status })
    }
  }
}

/// Sends a GET request. A response with an unsuccessful status is not an error, only a failure to get one is.
pub async fn get(url: &str, headers: &[(&str, String)]) -> Result<Response, CargoError> {
  let mut request = Client::new().get(url).header("User-Agent", USER_AGENT);
  for (name, value) in headers {
    request = request.header(*name, value.as_str());
  }

  let response = request.send().await.map_err(|source| CargoError::Request { url: url.to_string(), source })?;
  let status = response.status().as_u16();
  let body =
    response.into_body().text().await.map_err(|source| CargoError::ReadBody { url: url.to_string(), source })?;
  Ok(Response { status, body })
}

/// Headers for the GitHub API, authenticated if the plugin was configured with a `githubToken`.
///
/// Unauthenticated requests are limited to 60 an hour. The token is only ever sent to the GitHub API.
pub fn github_api_headers() -> Vec<(&'static str, String)> {
  let mut headers = vec![("Accept", "application/vnd.github+json".to_string())];
  let token = config::get().and_then(|config| config.github_token.as_deref());
  if let Some(token) = token {
    headers.push(("Authorization", format!("Bearer {token}")));
  }
  headers
}

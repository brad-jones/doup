//! Runs the short lived containers that guests request through `doup:plugin/host-api`.
//!
//! Containers are managed with [testcontainers], which despite the name is simply an ergonomic
//! wrapper around the Docker API that suits our "start, wait for exit, collect output, remove"
//! lifecycle. The usual `DOCKER_HOST` etc. environment variables are honoured.

use super::state::host_api::{self, RunOutput, RunRequest};
use std::path::{Path, PathBuf};
use std::time::Duration;
use testcontainers::core::logs::LogFrame;
use testcontainers::core::wait::ExitWaitStrategy;
use testcontainers::core::{AccessMode, Mount, WaitFor};
use testcontainers::runners::AsyncRunner;
use testcontainers::{ContainerAsync, GenericImage, ImageExt};

/// Where the guest's workspace is mounted inside the container.
const WORKSPACE_DIR: &str = "/workspace";

/// Where the guest's `/container-init` dir is mounted inside the container.
const INIT_DIR: &str = "/container-init";

/// Where the cache dir is mounted inside the container.
const CACHE_DIR: &str = "/container-cache";

/// The longest a container may take, including pulling its image, before it is killed.
const RUN_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// How much of a failed command's stderr is included in the error returned to the guest.
const MAX_ERROR_STDERR: usize = 4096;

/// Every way running a container can fail.
#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
  #[error("running containers is not permitted here")]
  NotPermitted,

  #[error("a container command must be given")]
  EmptyCommand,

  /// The container could not be created, started or inspected, `action` describes which.
  /// Boxed as a `TestcontainersError` is large and would bloat every `Result`.
  #[error("failed to {action} {image}")]
  Runtime { image: String, action: &'static str, source: Box<testcontainers::TestcontainersError> },

  /// The command ran but did not exit successfully, `code` is none if the runtime didn't report one.
  #[error(
    "`{command}` in {image} exited with code {}",
    code.map_or_else(|| "unknown".to_string(), |code| code.to_string())
  )]
  Exited { image: String, command: String, code: Option<i64>, stdout: String, stderr: String },
}

/// The guest only sees `run-error`, which can't nest, so a runtime failure's causes are flattened into its message.
impl From<ContainerError> for host_api::RunError {
  fn from(error: ContainerError) -> Self {
    match error {
      ContainerError::NotPermitted => Self::NotPermitted,
      ContainerError::EmptyCommand => Self::InvalidRequest(error.to_string()),
      ContainerError::Runtime { .. } => Self::Runtime(error_chain(&error)),
      ContainerError::Exited { code, stdout, stderr, .. } => {
        Self::Exited(host_api::RunExit { code, stdout, stderr: tail(stderr.trim_end(), MAX_ERROR_STDERR).to_string() })
      }
    }
  }
}

/// Whether a session's guest may run containers via `doup:plugin/host-api`.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum ContainerPolicy {
  /// Every `run` is rejected.
  #[default]
  Denied,

  /// Containers may be run, with these host directories mounted into them.
  Allowed(ContainerMounts),
}

/// The host directories mounted into every container a guest runs.
#[derive(Debug, Clone, PartialEq)]
pub struct ContainerMounts {
  /// Mounted at `/workspace`, this should be the same dir the guest sees as `/workspace`.
  pub workspace: PathBuf,

  /// Should match the permissions the guest has to `/workspace`.
  pub workspace_access: AccessMode,

  /// Mounted at `/container-init`, this should be the same dir the guest sees as `/container-init`.
  pub init: PathBuf,

  /// How containers may access `/container-init`, usually read-only as it only holds what the guest hands to them.
  pub init_access: AccessMode,

  /// Mounted read-write at `/container-cache`. Unlike the other dirs the guest has no view of it, it's
  /// only seen by containers, so whatever one writes (eg: downloads) is there for the next.
  /// The host attaches no meaning to the contents, how long this dir lives is up to whoever creates it.
  pub cache: PathBuf,
}

/// Runs `request` to completion, as permitted by `policy`.
pub async fn run(policy: &ContainerPolicy, request: RunRequest) -> Result<RunOutput, ContainerError> {
  let ContainerPolicy::Allowed(mounts) = policy else {
    return Err(ContainerError::NotPermitted);
  };
  if request.command.is_empty() {
    return Err(ContainerError::EmptyCommand);
  }

  let container = start(mounts, &request).await.map_err(runtime_error(&request, "run"))?;
  let output = collect(&container, &request).await;
  if let Err(err) = container.rm().await {
    eprintln!("warning: failed to remove container for {}: {err}", request.image);
  }
  output
}

/// Creates & starts the container, returning once its command has exited.
async fn start(
  mounts: &ContainerMounts,
  request: &RunRequest,
) -> testcontainers::core::error::Result<ContainerAsync<GenericImage>> {
  let (name, tag) = split_image(&request.image);
  let mut image = GenericImage::new(name, tag).with_wait_for(WaitFor::exit(ExitWaitStrategy::new()));
  if let Some(entrypoint) = &request.entrypoint {
    image = image.with_entrypoint(entrypoint);
  }

  let prefix = format!("[{}]", request.image);
  let mut container = image
    .with_cmd(&request.command)
    .with_working_dir(request.working_dir.as_deref().unwrap_or(WORKSPACE_DIR))
    .with_mount(Mount::bind_mount(path_str(&mounts.workspace), WORKSPACE_DIR).with_access_mode(mounts.workspace_access))
    .with_mount(Mount::bind_mount(path_str(&mounts.init), INIT_DIR).with_access_mode(mounts.init_access))
    .with_mount(Mount::bind_mount(path_str(&mounts.cache), CACHE_DIR))
    .with_label("dev.doup.managed", "true")
    .with_startup_timeout(RUN_TIMEOUT)
    .with_log_consumer(move |frame: &LogFrame| stream_log(&prefix, frame));
  for (key, value) in &request.env {
    container = container.with_env_var(key, value);
  }
  if let Some(user) = workspace_owner(&mounts.workspace) {
    container = container.with_user(user);
  }

  container.start().await
}

/// Reads the output of an exited container, failing if its command exited with a non-zero code.
async fn collect(container: &ContainerAsync<GenericImage>, request: &RunRequest) -> Result<RunOutput, ContainerError> {
  let exit_code = container.exit_code().await.map_err(runtime_error(request, "get the exit code of"))?;
  let stdout = container.stdout_to_vec().await.map_err(runtime_error(request, "read the stdout of"))?;
  let stderr = container.stderr_to_vec().await.map_err(runtime_error(request, "read the stderr of"))?;
  let stdout = String::from_utf8_lossy(&stdout).into_owned();
  let stderr = String::from_utf8_lossy(&stderr).into_owned();

  match exit_code {
    Some(0) => Ok(RunOutput { stdout, stderr }),
    code => Err(ContainerError::Exited {
      image: request.image.clone(),
      command: request.command.join(" "),
      code,
      stdout,
      stderr,
    }),
  }
}

/// Wraps a testcontainers error as a [`ContainerError::Runtime`] while trying to `action` the request's image.
fn runtime_error(
  request: &RunRequest,
  action: &'static str,
) -> impl FnOnce(testcontainers::TestcontainersError) -> ContainerError {
  let image = request.image.clone();
  move |source| ContainerError::Runtime { image, action, source: Box::new(source) }
}

/// Renders `error` & every one of its causes, separated by `: `.
fn error_chain(error: &dyn std::error::Error) -> String {
  std::iter::successors(Some(error), |err| err.source()).map(ToString::to_string).collect::<Vec<_>>().join(": ")
}

/// Splits an image reference into the name & tag testcontainers wants, which it rejoins with a `:`.
///
/// The tag is whatever follows the last `:` of the final path segment, so a digest such as
/// `rust@sha256:abc` splits into `rust@sha256` & `abc`, which rejoin to the original reference.
fn split_image(image: &str) -> (&str, &str) {
  let last_segment = image.rfind('/').map_or(0, |i| i + 1);
  match image[last_segment..].rfind(':') {
    Some(i) => (&image[..last_segment + i], &image[last_segment + i + 1..]),
    None => (image, "latest"),
  }
}

/// Docker wants mount sources as strings, a lossy conversion only matters for non UTF-8 paths.
fn path_str(path: &Path) -> String {
  path.to_string_lossy().into_owned()
}

/// The `uid:gid` owning `workspace`, so files the container creates there aren't owned by root.
#[cfg(unix)]
fn workspace_owner(workspace: &Path) -> Option<String> {
  use std::os::unix::fs::MetadataExt;
  std::fs::metadata(workspace).ok().map(|m| format!("{}:{}", m.uid(), m.gid()))
}

/// Docker Desktop maps file ownership itself on other platforms.
#[cfg(not(unix))]
fn workspace_owner(_workspace: &Path) -> Option<String> {
  None
}

/// Echoes container output to our stderr, so the user can follow along.
fn stream_log(prefix: &str, frame: &LogFrame) {
  let bytes = match frame {
    LogFrame::StdOut(bytes) | LogFrame::StdErr(bytes) => bytes,
  };
  for line in String::from_utf8_lossy(bytes).lines() {
    eprintln!("{prefix} {line}");
  }
}

/// The last `max` bytes of `text`, adjusted to a char boundary.
fn tail(text: &str, max: usize) -> &str {
  let mut start = text.len().saturating_sub(max);
  while !text.is_char_boundary(start) {
    start += 1;
  }
  &text[start..]
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn images_are_split_into_name_and_tag() {
    assert_eq!(split_image("rust"), ("rust", "latest"));
    assert_eq!(split_image("rust:slim"), ("rust", "slim"));
    assert_eq!(split_image("docker.io/library/rust:1.90-slim"), ("docker.io/library/rust", "1.90-slim"));
    assert_eq!(split_image("localhost:5000/rust"), ("localhost:5000/rust", "latest"));
    assert_eq!(split_image("localhost:5000/rust:slim"), ("localhost:5000/rust", "slim"));
    assert_eq!(split_image("rust@sha256:abc"), ("rust@sha256", "abc"));
  }

  #[test]
  fn tail_keeps_the_end_on_a_char_boundary() {
    assert_eq!(tail("hello", 10), "hello");
    assert_eq!(tail("hello", 3), "llo");
    assert_eq!(tail("aé", 1), "");
  }

  fn request(command: &[&str]) -> RunRequest {
    RunRequest {
      image: "docker.io/library/busybox:stable".to_string(),
      entrypoint: None,
      command: command.iter().map(ToString::to_string).collect(),
      env: vec![("GREETING".to_string(), "hello".to_string())],
      working_dir: None,
    }
  }

  /// Whether a container runtime is reachable, tests that need one are skipped otherwise.
  async fn docker_available() -> bool {
    let Ok(docker) = testcontainers::bollard::Docker::connect_with_defaults() else { return false };
    let available = docker.ping().await.is_ok();
    if !available {
      eprintln!("skipped, docker is unavailable");
    }
    available
  }

  #[tokio::test]
  async fn runs_are_denied_by_default() {
    let err = run(&ContainerPolicy::Denied, request(&["true"])).await.unwrap_err();
    assert!(matches!(err, ContainerError::NotPermitted), "{err}");
    assert!(matches!(host_api::RunError::from(err), host_api::RunError::NotPermitted));
  }

  #[test]
  fn exits_keep_the_tail_of_stderr_for_the_guest() {
    let err = ContainerError::Exited {
      image: "busybox".to_string(),
      command: "false".to_string(),
      code: Some(1),
      stdout: "out".to_string(),
      stderr: format!("{}tail\n", "x".repeat(MAX_ERROR_STDERR)),
    };
    assert_eq!(err.to_string(), "`false` in busybox exited with code 1");
    let host_api::RunError::Exited(exit) = host_api::RunError::from(err) else { panic!("expected an exit") };
    assert_eq!((exit.code, exit.stdout.as_str(), exit.stderr.len()), (Some(1), "out", MAX_ERROR_STDERR));
    assert!(exit.stderr.ends_with("tail"), "{}", exit.stderr);
  }

  #[tokio::test]
  async fn runs_against_the_mounted_dirs() {
    if !docker_available().await {
      return;
    }
    let (workspace, init, cache) =
      (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    std::fs::write(init.path().join("run.sh"), "echo \"$GREETING from $(pwd)\" | tee out.txt\n").unwrap();
    let policy = ContainerPolicy::Allowed(ContainerMounts {
      workspace: workspace.path().to_path_buf(),
      workspace_access: AccessMode::ReadWrite,
      init: init.path().to_path_buf(),
      init_access: AccessMode::ReadOnly,
      cache: cache.path().to_path_buf(),
    });

    let output = run(&policy, request(&["sh", "/container-init/run.sh"])).await.unwrap();
    assert_eq!(output.stdout, "hello from /workspace\n");
    let out = workspace.path().join("out.txt");
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "hello from /workspace\n");
    #[cfg(unix)]
    {
      use std::os::unix::fs::MetadataExt;
      let (dir, file) = (std::fs::metadata(workspace.path()).unwrap(), std::fs::metadata(&out).unwrap());
      assert_eq!((file.uid(), file.gid()), (dir.uid(), dir.gid()));
    }

    let err = run(&policy, request(&["touch", "/container-init/new"])).await.unwrap_err();
    assert!(matches!(err, ContainerError::Exited { code: Some(1), .. }), "{err}");
    assert!(!init.path().join("new").exists());

    // The cache is writable & persists between runs.
    run(&policy, request(&["sh", "-c", "echo cached > /container-cache/hit"])).await.unwrap();
    let output = run(&policy, request(&["cat", "/container-cache/hit"])).await.unwrap();
    assert_eq!(output.stdout, "cached\n");
  }

  #[tokio::test]
  async fn a_failed_command_is_an_error() {
    if !docker_available().await {
      return;
    }
    let (workspace, init, cache) =
      (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let policy = ContainerPolicy::Allowed(ContainerMounts {
      workspace: workspace.path().to_path_buf(),
      workspace_access: AccessMode::ReadOnly,
      init: init.path().to_path_buf(),
      init_access: AccessMode::ReadOnly,
      cache: cache.path().to_path_buf(),
    });

    let err = run(&policy, request(&["sh", "-c", "echo oops >&2; exit 3"])).await.unwrap_err();
    assert!(matches!(&err, ContainerError::Exited { code: Some(3), stderr, .. } if stderr == "oops\n"), "{err}");
  }
}

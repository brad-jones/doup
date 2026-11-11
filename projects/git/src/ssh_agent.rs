use crate::{Auth, GitError};
use rsa::pkcs1v15::SigningKey;
use rsa::rand_core::OsRng;
use rsa::sha2::{Sha256, Sha512};
use rsa::signature::{RandomizedSigner as _, SignatureEncoding as _, Signer as _};
use ssh_agent_lib::agent::{Session, listen};
use ssh_agent_lib::error::AgentError;
use ssh_agent_lib::proto::{Identity, RSA_SHA2_256, RSA_SHA2_512, SignRequest};
use ssh_key::private::{KeypairData, RsaKeypair};
use ssh_key::public::KeyData;
use ssh_key::{Algorithm, HashAlg, Mpint, PrivateKey, Signature};
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tempfile::TempDir;
use tokio::task::AbortHandle;

/// An ssh-agent served from this process, so the private key never touches disk.
///
/// Only the public key and the socket live on disk (owner-only), so a killed process leaks no secret.
#[derive(Debug)]
pub(crate) struct SshAgent {
  socket: PathBuf,
  public_key: PathBuf,
  listener: AbortHandle,
  _dir: TempDir,
}

impl SshAgent {
  /// The agent to use for `auth`, if it is SSH auth.
  pub(crate) fn for_auth(auth: Option<&Auth>) -> Result<Option<Self>, GitError> {
    match auth {
      Some(Auth::Ssh { key }) => Self::new(key).map(Some),
      _ => Ok(None),
    }
  }

  /// Starts serving `key` (an unencrypted OpenSSH private key); must be called within a tokio runtime.
  pub(crate) fn new(key: &str) -> Result<Self, GitError> {
    let session = KeySession::new(key)?;
    let dir = tempfile::Builder::new()
      .prefix("doup-ssh-agent-")
      .tempdir()
      .map_err(GitError::SshAgent)?;
    let public_key = dir.path().join("id.pub");
    std::fs::write(&public_key, &session.public_openssh).map_err(GitError::SshAgent)?;
    let (socket, listener) = serve(dir.path(), session).map_err(GitError::SshAgent)?;
    Ok(Self {
      socket,
      public_key,
      listener,
      _dir: dir,
    })
  }

  /// A `GIT_SSH_COMMAND` that only offers this agent's key and never prompts.
  ///
  /// `-i` takes the public key so ssh asks the agent for it; the socket is inlined because grit
  /// does not pass its environment on to ssh.
  pub(crate) fn ssh_command(&self) -> Result<String, GitError> {
    Ok(format!(
      "SSH_AUTH_SOCK={} {} -o IdentityAgent=SSH_AUTH_SOCK -o IdentitiesOnly=yes -i {} \
       -o BatchMode=yes -o StrictHostKeyChecking=accept-new",
      quote(&self.socket)?,
      quote(&ssh_program()?)?,
      quote(&self.public_key)?,
    ))
  }
}

impl Drop for SshAgent {
  fn drop(&mut self) {
    self.listener.abort();
  }
}

fn quote(path: &Path) -> Result<String, GitError> {
  let invalid = || GitError::InvalidSshPath {
    path: path.to_path_buf(),
  };
  let text = path.to_str().ok_or_else(invalid)?;
  Ok(shlex::try_quote(text).map_err(|_| invalid())?.into_owned())
}

#[cfg(unix)]
fn ssh_program() -> Result<PathBuf, GitError> {
  Ok(PathBuf::from("ssh"))
}

/// Git for Windows bundles an MSYS ssh that cannot reach named pipes, so use Windows' own OpenSSH.
#[cfg(windows)]
fn ssh_program() -> Result<PathBuf, GitError> {
  let root = std::env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
  let path = Path::new(&root).join(r"System32\OpenSSH\ssh.exe");
  if path.is_file() {
    Ok(path)
  } else {
    Err(GitError::SshNotFound { path })
  }
}

#[cfg(unix)]
fn serve(dir: &Path, session: KeySession) -> io::Result<(PathBuf, AbortHandle)> {
  let socket = dir.join("agent.sock");
  let listener = tokio::net::UnixListener::bind(&socket)?;
  let task = tokio::spawn(listen(listener, session));
  Ok((socket, task.abort_handle()))
}

#[cfg(windows)]
fn serve(_dir: &Path, session: KeySession) -> io::Result<(PathBuf, AbortHandle)> {
  use rsa::rand_core::RngCore as _;
  use ssh_agent_lib::agent::NamedPipeListener;

  let mut suffix = [0u8; 16];
  OsRng.fill_bytes(&mut suffix);
  let suffix: String = suffix.iter().map(|byte| format!("{byte:02x}")).collect();
  let pipe = PathBuf::from(format!(r"\\.\pipe\doup-ssh-agent-{suffix}"));
  let listener = NamedPipeListener::bind(pipe.as_os_str())?;
  let task = tokio::spawn(listen(listener, session));
  Ok((pipe, task.abort_handle()))
}

/// Serves the agent protocol for a single key; cloned for every connection.
#[derive(Clone)]
struct KeySession {
  public: KeyData,
  public_openssh: String,
  signer: Arc<KeySigner>,
}

enum KeySigner {
  /// RSA is signed directly so the hash the client asks for can be honoured.
  Rsa(rsa::RsaPrivateKey),
  Ssh(PrivateKey),
}

impl KeySession {
  fn new(text: &str) -> Result<Self, GitError> {
    let key = PrivateKey::from_openssh(text).map_err(GitError::InvalidSshKey)?;
    if key.is_encrypted() {
      return Err(GitError::EncryptedSshKey);
    }
    let public = key.public_key().key_data().clone();
    let public_openssh = key
      .public_key()
      .to_openssh()
      .map_err(GitError::InvalidSshKey)?
      + "\n";
    let signer = match key.key_data() {
      KeypairData::Rsa(rsa) => {
        KeySigner::Rsa(rsa_private_key(rsa).map_err(GitError::InvalidSshKey)?)
      }
      KeypairData::Ed25519(_) | KeypairData::Ecdsa(_) => KeySigner::Ssh(key),
      _ => {
        return Err(GitError::UnsupportedSshKey {
          algorithm: key.algorithm().to_string(),
        });
      }
    };
    Ok(Self {
      public,
      public_openssh,
      signer: Arc::new(signer),
    })
  }
}

/// Built by hand because ssh-key 0.6.7's conversion passes `p` where it should pass `q`.
fn rsa_private_key(key: &RsaKeypair) -> Result<rsa::RsaPrivateKey, ssh_key::Error> {
  let uint = |mpint: &Mpint| rsa::BigUint::try_from(mpint);
  Ok(rsa::RsaPrivateKey::from_components(
    uint(&key.public.n)?,
    uint(&key.public.e)?,
    uint(&key.private.d)?,
    vec![uint(&key.private.p)?, uint(&key.private.q)?],
  )?)
}

impl KeySigner {
  fn sign(&self, data: &[u8], flags: u32) -> Result<Signature, AgentError> {
    let key = match self {
      Self::Ssh(key) => return key.try_sign(data).map_err(AgentError::other),
      Self::Rsa(key) => key.clone(),
    };
    let (hash, signature) = if flags & RSA_SHA2_512 != 0 {
      let signature = SigningKey::<Sha512>::new(key).try_sign_with_rng(&mut OsRng, data);
      (HashAlg::Sha512, signature)
    } else if flags & RSA_SHA2_256 != 0 {
      let signature = SigningKey::<Sha256>::new(key).try_sign_with_rng(&mut OsRng, data);
      (HashAlg::Sha256, signature)
    } else {
      return Err(AgentError::other(io::Error::other(
        "ssh-rsa (SHA-1) signatures are not supported",
      )));
    };
    let signature = signature.map_err(AgentError::other)?;
    Signature::new(Algorithm::Rsa { hash: Some(hash) }, signature.to_vec())
      .map_err(AgentError::other)
  }
}

#[ssh_agent_lib::async_trait]
impl Session for KeySession {
  async fn request_identities(&mut self) -> Result<Vec<Identity>, AgentError> {
    Ok(vec![Identity {
      credential: self.public.clone().into(),
      comment: "doup".into(),
    }])
  }

  async fn sign(&mut self, request: SignRequest) -> Result<Signature, AgentError> {
    if request.credential.key_data() != &self.public {
      return Err(AgentError::Failure);
    }
    self.signer.sign(&request.data, request.flags)
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::test_support::generate_ssh_key;
  use rsa::signature::Verifier;
  use ssh_key::PublicKey;

  async fn client(agent: &SshAgent) -> Box<dyn Session> {
    #[cfg(unix)]
    let stream = tokio::net::UnixStream::connect(&agent.socket)
      .await
      .unwrap();
    #[cfg(windows)]
    let stream = tokio::net::windows::named_pipe::ClientOptions::new()
      .open(&agent.socket)
      .unwrap();
    Box::new(ssh_agent_lib::client::Client::new(stream))
  }

  fn public_key(key: &str) -> PublicKey {
    PrivateKey::from_openssh(key).unwrap().public_key().clone()
  }

  async fn sign(agent: &SshAgent, public: &PublicKey, flags: u32) -> Result<Signature, AgentError> {
    client(agent)
      .await
      .sign(SignRequest {
        credential: public.key_data().clone().into(),
        data: b"data".to_vec(),
        flags,
      })
      .await
  }

  #[tokio::test]
  async fn rejects_keys_it_cannot_serve() {
    let err = SshAgent::new("not a key").unwrap_err();
    assert!(matches!(err, GitError::InvalidSshKey(_)), "{err:?}");

    let encrypted = generate_ssh_key("ed25519", "hunter2");
    let err = SshAgent::new(&encrypted).unwrap_err();
    assert!(matches!(err, GitError::EncryptedSshKey), "{err:?}");
  }

  #[tokio::test]
  async fn for_auth_only_starts_an_agent_for_ssh() {
    let https = Auth::Https {
      user: "u".into(),
      pass: "p".into(),
    };
    let ssh = Auth::Ssh {
      key: generate_ssh_key("ed25519", ""),
    };
    assert!(SshAgent::for_auth(None).unwrap().is_none());
    assert!(SshAgent::for_auth(Some(&https)).unwrap().is_none());
    assert!(SshAgent::for_auth(Some(&ssh)).unwrap().is_some());
  }

  #[tokio::test]
  async fn lists_only_its_own_key() {
    let key = generate_ssh_key("ed25519", "");
    let agent = SshAgent::new(&key).unwrap();
    let identities = client(&agent).await.request_identities().await.unwrap();
    assert_eq!(identities.len(), 1);
    assert_eq!(
      identities[0].credential.key_data(),
      public_key(&key).key_data()
    );
  }

  #[tokio::test]
  async fn signs_with_every_supported_key_type() {
    for (kind, flags, hash) in [
      ("ed25519", 0, None),
      ("ecdsa", 0, None),
      ("rsa", RSA_SHA2_512, Some(HashAlg::Sha512)),
      ("rsa", RSA_SHA2_256, Some(HashAlg::Sha256)),
    ] {
      let key = generate_ssh_key(kind, "");
      let public = public_key(&key);
      let agent = SshAgent::new(&key).unwrap();
      let signature = sign(&agent, &public, flags).await.unwrap();
      Verifier::verify(public.key_data(), b"data", &signature).unwrap();
      if let Some(hash) = hash {
        assert_eq!(signature.algorithm(), Algorithm::Rsa { hash: Some(hash) });
      }
    }
  }

  #[tokio::test]
  async fn refuses_sha1_rsa_and_keys_it_does_not_hold() {
    let key = generate_ssh_key("rsa", "");
    let agent = SshAgent::new(&key).unwrap();
    assert!(sign(&agent, &public_key(&key), 0).await.is_err());

    let other = public_key(&generate_ssh_key("ed25519", ""));
    assert!(sign(&agent, &other, 0).await.is_err());
  }

  #[tokio::test]
  async fn keeps_only_public_files_on_disk_and_removes_them_on_drop() {
    let key = generate_ssh_key("ed25519", "");
    let agent = SshAgent::new(&key).unwrap();
    let dir = agent._dir.path().to_path_buf();

    let mut names: Vec<_> = std::fs::read_dir(&dir)
      .unwrap()
      .map(|entry| entry.unwrap().file_name().into_string().unwrap())
      .collect();
    names.sort();
    #[cfg(unix)]
    assert_eq!(names, ["agent.sock", "id.pub"]);
    #[cfg(windows)]
    assert_eq!(names, ["id.pub"]);
    assert_eq!(
      PublicKey::from_openssh(&std::fs::read_to_string(&agent.public_key).unwrap()).unwrap(),
      public_key(&key)
    );

    drop(agent);
    assert!(!dir.exists());
  }

  #[tokio::test]
  async fn ssh_command_only_offers_the_agent_key() {
    let agent = SshAgent::new(&generate_ssh_key("ed25519", "")).unwrap();
    let command = agent.ssh_command().unwrap();
    let socket = format!("SSH_AUTH_SOCK={}", agent.socket.to_str().unwrap());
    let program = ssh_program().unwrap();
    assert_eq!(
      shlex::split(&command).unwrap(),
      [
        socket.as_str(),
        program.to_str().unwrap(),
        "-o",
        "IdentityAgent=SSH_AUTH_SOCK",
        "-o",
        "IdentitiesOnly=yes",
        "-i",
        agent.public_key.to_str().unwrap(),
        "-o",
        "BatchMode=yes",
        "-o",
        "StrictHostKeyChecking=accept-new",
      ]
    );
  }

  /// Real OpenSSH tooling can sign with the agent's key given only the public key.
  #[cfg(unix)]
  #[tokio::test(flavor = "multi_thread")]
  async fn openssh_signs_through_the_agent() {
    use std::process::{Command, Stdio};

    for kind in ["ed25519", "ecdsa", "rsa"] {
      let agent = SshAgent::new(&generate_ssh_key(kind, "")).unwrap();
      let message = agent._dir.path().join("message");
      std::fs::write(&message, "data").unwrap();

      let output = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", "test", "-f"])
        .arg(&agent.public_key)
        .arg(&message)
        .env("SSH_AUTH_SOCK", &agent.socket)
        .stdin(Stdio::null())
        .output()
        .unwrap();
      assert!(output.status.success(), "{kind}: {output:?}");

      let output = Command::new("ssh-keygen")
        .args(["-Y", "check-novalidate", "-n", "test", "-s"])
        .arg(message.with_extension("sig"))
        .stdin(std::fs::File::open(&message).unwrap())
        .output()
        .unwrap();
      assert!(output.status.success(), "{kind}: {output:?}");
    }
  }
}

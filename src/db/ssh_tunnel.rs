use std::fmt;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use russh::client::{self, AuthResult};
use russh::keys::{
    HashAlg, PrivateKeyWithHashAlg,
    agent::{AgentIdentity, client::AgentClient},
    load_secret_key,
    ssh_key::PublicKey,
};
use russh::{ChannelMsg, Disconnect};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::timeout;

use crate::models::connection::{ConnectionDetails, SshAuthMethod, SshTunnelConfig};

#[derive(Debug)]
pub enum SshTunnelError {
    MissingConfig,
    UntrustedHostKey(String),
    HostKeyChanged { expected: String, actual: String },
    AuthenticationFailed,
    AgentUnavailable(String),
    AgentHasNoIdentities,
    TimedOut,
    Io(std::io::Error),
    Russh(russh::Error),
    Key(russh::keys::Error),
    AgentAuth(russh::AgentAuthError),
}

impl fmt::Display for SshTunnelError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingConfig => write!(formatter, "SSH tunnel configuration is missing"),
            Self::UntrustedHostKey(fingerprint) => write!(
                formatter,
                "SSH host key is not trusted yet. Fingerprint: {fingerprint}"
            ),
            Self::HostKeyChanged { expected, actual } => write!(
                formatter,
                "SSH host key changed. Expected {expected}, got {actual}"
            ),
            Self::AuthenticationFailed => write!(formatter, "SSH authentication failed"),
            Self::AgentUnavailable(error) => {
                write!(formatter, "SSH agent is not available: {error}")
            }
            Self::AgentHasNoIdentities => write!(formatter, "SSH agent has no loaded identities"),
            Self::TimedOut => write!(formatter, "SSH connection timed out"),
            Self::Io(error) => write!(formatter, "{error}"),
            Self::Russh(error) => write!(formatter, "{error}"),
            Self::Key(error) => write!(formatter, "{error}"),
            Self::AgentAuth(error) => write!(
                formatter,
                "SSH agent signing failed. The agent may have refused the request or confirmation was denied: {error}"
            ),
        }
    }
}

const SSH_OPERATION_TIMEOUT: Duration = Duration::from_secs(15);

impl std::error::Error for SshTunnelError {}

impl From<std::io::Error> for SshTunnelError {
    fn from(error: std::io::Error) -> Self {
        Self::Io(error)
    }
}

impl From<russh::Error> for SshTunnelError {
    fn from(error: russh::Error) -> Self {
        Self::Russh(error)
    }
}

impl From<russh::keys::Error> for SshTunnelError {
    fn from(error: russh::keys::Error) -> Self {
        Self::Key(error)
    }
}

impl From<russh::AgentAuthError> for SshTunnelError {
    fn from(error: russh::AgentAuthError) -> Self {
        Self::AgentAuth(error)
    }
}

pub struct TunnelGuard {
    local_port: u16,
    task: JoinHandle<()>,
    forward_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
}

impl fmt::Debug for TunnelGuard {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("TunnelGuard")
            .field("local_port", &self.local_port)
            .finish_non_exhaustive()
    }
}

impl TunnelGuard {
    pub fn local_port(&self) -> u16 {
        self.local_port
    }
}

impl Drop for TunnelGuard {
    fn drop(&mut self) {
        self.task.abort();

        if let Ok(mut forward_tasks) = self.forward_tasks.lock() {
            for task in forward_tasks.drain(..) {
                task.abort();
            }
        }
    }
}

#[derive(Clone)]
struct HostKeyVerifier {
    expected_fingerprint: Option<String>,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
}

impl client::Handler for HostKeyVerifier {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        server_public_key: &PublicKey,
    ) -> Result<bool, Self::Error> {
        let fingerprint = server_public_key.fingerprint(HashAlg::Sha256).to_string();

        if let Ok(mut observed) = self.observed_fingerprint.lock() {
            *observed = Some(fingerprint.clone());
        }

        Ok(self
            .expected_fingerprint
            .as_ref()
            .is_none_or(|expected| expected == &fingerprint))
    }
}

pub async fn start_tunnel(details: &ConnectionDetails) -> Result<TunnelGuard, SshTunnelError> {
    let config = details
        .saved
        .ssh_tunnel
        .as_ref()
        .ok_or(SshTunnelError::MissingConfig)?;

    let observed_fingerprint = Arc::new(Mutex::new(None));
    let verifier = HostKeyVerifier {
        expected_fingerprint: config.host_key_fingerprint.clone(),
        observed_fingerprint: observed_fingerprint.clone(),
    };

    let mut session = timeout(SSH_OPERATION_TIMEOUT, connect_ssh(config, verifier))
        .await
        .map_err(|_| SshTunnelError::TimedOut)??;
    validate_observed_host_key(config, observed_fingerprint)?;

    timeout(
        SSH_OPERATION_TIMEOUT,
        authenticate(&mut session, config, details),
    )
    .await
    .map_err(|_| SshTunnelError::TimedOut)??;

    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let local_port = listener.local_addr()?.port();
    let database_host = details.saved.host.clone();
    let database_port = details.saved.port;

    let forward_tasks = Arc::new(Mutex::new(Vec::new()));
    let task_forward_tasks = forward_tasks.clone();

    let task = tokio::spawn(async move {
        accept_forwarded_connections(
            listener,
            session,
            database_host,
            database_port,
            task_forward_tasks,
        )
        .await;
    });

    Ok(TunnelGuard {
        local_port,
        task,
        forward_tasks,
    })
}

async fn connect_ssh(
    config: &SshTunnelConfig,
    verifier: HostKeyVerifier,
) -> Result<client::Handle<HostKeyVerifier>, SshTunnelError> {
    let ssh_config = client::Config {
        nodelay: true,
        ..Default::default()
    };

    let session = client::connect(
        Arc::new(ssh_config),
        (config.host.as_str(), config.port),
        verifier,
    )
    .await?;

    Ok(session)
}

fn validate_observed_host_key(
    config: &SshTunnelConfig,
    observed_fingerprint: Arc<Mutex<Option<String>>>,
) -> Result<(), SshTunnelError> {
    let actual = observed_fingerprint
        .lock()
        .ok()
        .and_then(|fingerprint| fingerprint.clone())
        .ok_or_else(|| SshTunnelError::UntrustedHostKey(String::new()))?;

    match config.host_key_fingerprint.as_ref() {
        Some(expected) if expected == &actual => Ok(()),
        Some(expected) => Err(SshTunnelError::HostKeyChanged {
            expected: expected.clone(),
            actual,
        }),
        None => Err(SshTunnelError::UntrustedHostKey(actual)),
    }
}

async fn authenticate(
    session: &mut client::Handle<HostKeyVerifier>,
    config: &SshTunnelConfig,
    details: &ConnectionDetails,
) -> Result<(), SshTunnelError> {
    let result = match config.auth_method {
        SshAuthMethod::Password => {
            session
                .authenticate_password(&config.username, &details.ssh_password)
                .await?
        }
        SshAuthMethod::PrivateKey => {
            let passphrase = if details.ssh_key_passphrase.is_empty() {
                None
            } else {
                Some(details.ssh_key_passphrase.as_str())
            };
            let key = load_secret_key(&config.private_key_path, passphrase)?;
            let hash_alg = session.best_supported_rsa_hash().await?.flatten();

            session
                .authenticate_publickey(
                    &config.username,
                    PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
                )
                .await?
        }
        SshAuthMethod::Agent => authenticate_with_agent(session, &config.username).await?,
    };

    if matches!(result, AuthResult::Success) {
        return Ok(());
    }

    Err(SshTunnelError::AuthenticationFailed)
}

async fn authenticate_with_agent(
    session: &mut client::Handle<HostKeyVerifier>,
    username: &str,
) -> Result<AuthResult, SshTunnelError> {
    let mut agent = AgentClient::connect_env()
        .await
        .map_err(|e| SshTunnelError::AgentUnavailable(e.to_string()))?;

    let identities = agent.request_identities().await?;

    if identities.is_empty() {
        return Err(SshTunnelError::AgentHasNoIdentities);
    }

    let hash_alg = session.best_supported_rsa_hash().await?.flatten();

    for identity in &identities {
        let AgentIdentity::PublicKey { key, .. } = identity else {
            continue;
        };

        let result = session
            .authenticate_publickey_with(username, key.clone(), hash_alg, &mut agent)
            .await?;

        if matches!(result, AuthResult::Success) {
            return Ok(result);
        }
    }

    for identity in identities {
        let AgentIdentity::Certificate { certificate, .. } = identity else {
            continue;
        };

        let result = session
            .authenticate_certificate_with(username, certificate, hash_alg, &mut agent)
            .await?;

        if matches!(result, AuthResult::Success) {
            return Ok(result);
        }
    }

    Err(SshTunnelError::AuthenticationFailed)
}

async fn accept_forwarded_connections(
    listener: TcpListener,
    session: client::Handle<HostKeyVerifier>,
    database_host: String,
    database_port: u16,
    forward_tasks: Arc<Mutex<Vec<JoinHandle<()>>>>,
) {
    while let Ok((stream, originator_addr)) = listener.accept().await {
        let channel = session
            .channel_open_direct_tcpip(
                database_host.clone(),
                database_port.into(),
                originator_addr.ip().to_string(),
                originator_addr.port().into(),
            )
            .await;

        let Ok(channel) = channel else {
            continue;
        };

        let task = tokio::spawn(async move {
            let _ = forward_stream(stream, originator_addr, channel).await;
        });

        if let Ok(mut forward_tasks) = forward_tasks.lock() {
            forward_tasks.push(task);
        }
    }

    let _ = session
        .disconnect(Disconnect::ByApplication, "", "English")
        .await;
}

async fn forward_stream(
    mut stream: TcpStream,
    _originator_addr: SocketAddr,
    mut channel: russh::Channel<russh::client::Msg>,
) -> Result<(), SshTunnelError> {
    let mut stream_closed = false;
    let mut buffer = vec![0; 65536];

    loop {
        tokio::select! {
            read = stream.read(&mut buffer), if !stream_closed => {
                match read {
                    Ok(0) => {
                        stream_closed = true;
                        channel.eof().await?;
                    }
                    Ok(count) => channel.data(&buffer[..count]).await?,
                    Err(error) => return Err(error.into()),
                }
            }
            message = channel.wait() => {
                match message {
                    Some(ChannelMsg::Data { ref data }) => {
                        stream.write_all(data).await?;
                    }
                    Some(ChannelMsg::Eof) => {
                        if !stream_closed {
                            channel.eof().await?;
                        }
                        break;
                    }
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
        }
    }

    Ok(())
}

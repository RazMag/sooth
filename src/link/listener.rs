//! The agent's TLS listener for `axum::serve`. Handshakes run in their own
//! tasks (with a timeout), so one slow or silent client can't hold up the
//! others; each finished connection comes with the [`Peer`] it came from,
//! which handlers read through `ConnectInfo<Peer>`.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::connect_info::Connected;
use axum::serve::{IncomingStream, Listener};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;
use tokio_rustls::TlsAcceptor;
use tokio_rustls::server::TlsStream;

use super::Fingerprint;

/// Who is on the other end of a connection.
#[derive(Debug, Clone)]
pub struct Peer {
    pub addr: SocketAddr,
    /// Their client certificate's fingerprint (`None` only for the
    /// listener's own address).
    pub fingerprint: Option<Fingerprint>,
}

pub struct TlsListener {
    local: SocketAddr,
    ready: mpsc::Receiver<(TlsStream<TcpStream>, Peer)>,
    accept_task: tokio::task::JoinHandle<()>,
}

const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(10);

impl TlsListener {
    pub fn new(tcp: TcpListener, config: rustls::ServerConfig) -> std::io::Result<Self> {
        let local = tcp.local_addr()?;
        let acceptor = TlsAcceptor::from(Arc::new(config));
        let (tx, ready) = mpsc::channel(64);
        let accept_task = tokio::spawn(async move {
            loop {
                let (stream, addr) = match tcp.accept().await {
                    Ok(conn) => conn,
                    Err(e) => {
                        tracing::warn!(error = %e, "accept failed");
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        continue;
                    }
                };
                let acceptor = acceptor.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    match tokio::time::timeout(HANDSHAKE_TIMEOUT, acceptor.accept(stream)).await {
                        Ok(Ok(tls)) => {
                            let fingerprint = tls
                                .get_ref()
                                .1
                                .peer_certificates()
                                .and_then(|c| c.first())
                                .map(Fingerprint::of);
                            let _ = tx.send((tls, Peer { addr, fingerprint })).await;
                        }
                        Ok(Err(e)) => tracing::debug!(%addr, error = %e, "TLS handshake failed"),
                        Err(_) => tracing::debug!(%addr, "TLS handshake timed out"),
                    }
                });
            }
        });
        Ok(Self {
            local,
            ready,
            accept_task,
        })
    }
}

impl Drop for TlsListener {
    fn drop(&mut self) {
        self.accept_task.abort();
    }
}

impl Listener for TlsListener {
    type Io = TlsStream<TcpStream>;
    type Addr = Peer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        match self.ready.recv().await {
            Some(conn) => conn,
            // The accept task only ends when aborted, which happens on drop.
            None => std::future::pending().await,
        }
    }

    fn local_addr(&self) -> std::io::Result<Self::Addr> {
        Ok(Peer {
            addr: self.local,
            fingerprint: None,
        })
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for Peer {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        stream.remote_addr().clone()
    }
}

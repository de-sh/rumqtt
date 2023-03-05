use std::{io, net::SocketAddr, pin::Pin, sync::Arc, task::Poll};

use quinn::{Endpoint, RecvStream, SendStream, ServerConfig};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::rustls::ServerConfig as Crypto;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO = {0}")]
    Io(#[from] std::io::Error),
    #[error("Connection = {0}")]
    Connection(#[from] quinn::ConnectionError),
    #[error("No Incoming")]
    NoIncoming,
}

pub struct QuicListener {
    listener: Endpoint,
}

impl QuicListener {
    pub fn new(crypto: Crypto, addr: SocketAddr) -> Result<Self, Error> {
        let server_config = ServerConfig::with_crypto(Arc::new(crypto));
        let listener = Endpoint::server(server_config, addr)?;

        Ok(Self { listener })
    }

    pub async fn accept(&self) -> Result<(QuicNetwork, SocketAddr, Option<String>), Error> {
        let connecting = self.listener.accept().await.ok_or(Error::NoIncoming)?;
        let addr = connecting.remote_address();
        let connection = connecting.await?;
        // let certificate: Certificate = *connection.peer_identity().unwrap().downcast().unwrap();
        // let tenant_id = extract_tenant_id(&certificate.0).unwrap();
        let (tx, rx) = connection.accept_bi().await?;
        Ok((QuicNetwork { tx, rx }, addr, None))
    }
}

pub struct QuicNetwork {
    tx: SendStream,
    rx: RecvStream,
}

impl AsyncRead for QuicNetwork {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let s = Pin::into_inner(self);
        Pin::new(&mut s.rx).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicNetwork {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<Result<usize, std::io::Error>> {
        let s = Pin::into_inner(self);
        Pin::new(&mut s.tx).poll_write(cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let s = Pin::into_inner(self);
        Pin::new(&mut s.tx).poll_flush(cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), std::io::Error>> {
        let s = Pin::into_inner(self);
        Pin::new(&mut s.tx).poll_shutdown(cx)
    }
}

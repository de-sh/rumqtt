use std::{io, net::SocketAddr, pin::Pin, sync::Arc, task::Poll};

use quinn::{ClientConfig, Endpoint, RecvStream, SendStream};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio_rustls::rustls::ClientConfig as Crypto;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("IO = {0}")]
    Io(#[from] std::io::Error),
    #[error("Connect = {0}")]
    Connect(#[from] quinn::ConnectError),
    #[error("Connection = {0}")]
    Connection(#[from] quinn::ConnectionError),
}

pub struct QuicConnection {
    connection: Endpoint,
}

impl QuicConnection {
    pub fn new(addr: SocketAddr) -> Result<Self, Error> {
        let connection = Endpoint::client(addr)?;

        Ok(Self { connection })
    }

    pub async fn connect(
        &self,
        crypto: Crypto,
        addr: SocketAddr,
        server_name: &str,
    ) -> Result<QuicNetwork, Error> {
        let client_config = ClientConfig::new(Arc::new(crypto));
        let connection = self
            .connection
            .connect_with(client_config, addr, server_name)?
            .await?;
        let (tx, rx) = connection.accept_bi().await?;

        Ok(QuicNetwork { tx, rx })
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

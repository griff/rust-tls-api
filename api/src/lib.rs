//! Implementation neutral TLS API.

use std::error;
use std::fmt;
use std::future::Future;
use std::io;
use std::pin::Pin;
use std::result;
use std::task::Context;
use std::task::Poll;

pub mod async_as_sync;
pub mod runtime;

use runtime::{AsyncRead, AsyncWrite};

// Error

pub struct Error(Box<dyn error::Error + Send + Sync + 'static>);

/// An error returned from the TLS implementation.
impl Error {
    pub fn new<E: error::Error + 'static + Send + Sync>(e: E) -> Error {
        Error(Box::new(e))
    }

    pub fn new_other(message: &str) -> Error {
        Self::new(io::Error::new(io::ErrorKind::Other, message))
    }

    pub fn into_inner(self) -> Box<dyn error::Error + Send + Sync> {
        self.0
    }
}

impl error::Error for Error {
    fn source(&self) -> Option<&(dyn error::Error + 'static)> {
        self.0.source()
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Debug::fmt(&self.0, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl From<io::Error> for Error {
    fn from(err: io::Error) -> Error {
        Error::new(err)
    }
}

impl From<Error> for io::Error {
    fn from(err: Error) -> io::Error {
        io::Error::new(io::ErrorKind::Other, err)
    }
}

// Result

/// A typedef of the result type returned by many methods.
pub type Result<A> = result::Result<A, Error>;

pub enum CertificateFormat {
    DER,
    PEM,
}

// X.509 certificate
pub struct Certificate {
    pub bytes: Vec<u8>,
    pub format: CertificateFormat,
}

impl Certificate {
    pub fn from_der(der: Vec<u8>) -> Certificate {
        Certificate {
            bytes: der,
            format: CertificateFormat::DER,
        }
    }

    pub fn into_der(self) -> Option<Vec<u8>> {
        // TODO: there are methods to convert PEM->DER which might be used here
        match self.format {
            CertificateFormat::DER => Some(self.bytes),
            _ => None,
        }
    }
    pub fn into_pem(self) -> Option<Vec<u8>> {
        // TODO: there are methods to convert DER->PEM which might be used here
        match self.format {
            CertificateFormat::PEM => Some(self.bytes),
            _ => None,
        }
    }
}

pub trait TlsStreamImpl<S>:
    AsyncRead + AsyncWrite + Unpin + fmt::Debug + Send + Sync + 'static
{
    /// Get negotiated ALPN protocol.
    fn get_alpn_protocol(&self) -> Option<Vec<u8>>;

    fn get_mut(&mut self) -> &mut S;

    fn get_ref(&self) -> &S;
}

/// Since Rust has no HKT, it is not possible to declare something like
///
/// ```ignore
/// trait TlsConnector {
///     type <S> TlsStream<S> : TlsStreamImpl;
/// }
/// ```
///
/// So `TlsStream` is actually a box to concrete TLS implementation.
#[derive(Debug)]
pub struct TlsStream<S: 'static>(Box<dyn TlsStreamImpl<S> + 'static>);

impl<S: 'static> TlsStream<S> {
    pub fn new<I: TlsStreamImpl<S> + 'static>(imp: I) -> TlsStream<S> {
        TlsStream(Box::new(imp))
    }

    pub fn get_mut(&mut self) -> &mut S {
        self.0.get_mut()
    }

    pub fn get_ref(&self) -> &S {
        self.0.get_ref()
    }

    pub fn get_alpn_protocol(&self) -> Option<Vec<u8>> {
        self.0.get_alpn_protocol()
    }
}

impl<S> AsyncRead for TlsStream<S> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut [u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl<S> AsyncWrite for TlsStream<S> {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.0).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_flush(cx)
    }

    #[cfg(feature = "runtime-async-std")]
    fn poll_close(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_close(ctx)
    }

    #[cfg(feature = "runtime-tokio")]
    fn poll_shutdown(mut self: Pin<&mut Self>, ctx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_shutdown(ctx)
    }
}

/// A builder for `TlsConnector`s.
pub trait TlsConnectorBuilder: Sized + Sync + Send + 'static {
    type Connector: TlsConnector;

    type Underlying;

    fn underlying_mut(&mut self) -> &mut Self::Underlying;

    fn supports_alpn() -> bool;

    fn set_alpn_protocols(&mut self, protocols: &[&[u8]]) -> Result<()>;

    fn set_verify_hostname(&mut self, verify: bool) -> Result<()>;

    fn add_root_certificate(&mut self, cert: Certificate) -> Result<&mut Self>;

    fn build(self) -> Result<Self::Connector>;
}

/// A builder for client-side TLS connections.
pub trait TlsConnector: Sized + Sync + Send + 'static {
    type Builder: TlsConnectorBuilder<Connector = Self>;

    fn supports_alpn() -> bool {
        <Self::Builder as TlsConnectorBuilder>::supports_alpn()
    }

    fn builder() -> Result<Self::Builder>;

    fn connect<'a, S>(
        &'a self,
        domain: &'a str,
        stream: S,
    ) -> Pin<Box<dyn Future<Output = Result<TlsStream<S>>> + Send + 'a>>
    where
        S: AsyncRead + AsyncWrite + fmt::Debug + Unpin + Send + Sync + 'static;
}

/// A builder for `TlsAcceptor`s.
pub trait TlsAcceptorBuilder: Sized + Sync + Send + 'static {
    type Acceptor: TlsAcceptor;

    // Type of underlying builder
    type Underlying;

    fn supports_alpn() -> bool;

    fn set_alpn_protocols(&mut self, protocols: &[&[u8]]) -> Result<()>;

    fn underlying_mut(&mut self) -> &mut Self::Underlying;

    fn build(self) -> Result<Self::Acceptor>;
}

/// A builder for server-side TLS connections.
pub trait TlsAcceptor: Sized + Sync + Send + 'static {
    type Builder: TlsAcceptorBuilder<Acceptor = Self>;

    fn supports_alpn() -> bool {
        <Self::Builder as TlsAcceptorBuilder>::supports_alpn()
    }

    fn accept<'a, S>(
        &'a self,
        stream: S,
    ) -> Pin<Box<dyn Future<Output = Result<TlsStream<S>>> + Send + 'a>>
    where
        S: AsyncRead + AsyncWrite + fmt::Debug + Unpin + Send + Sync + 'static;
}

fn _check_kinds() {
    use std::net::TcpStream;

    fn assert_sync<T: Sync>() {}
    fn assert_send<T: Send>() {}
    fn assert_send_value<T: Send>(t: T) -> T {
        t
    }

    assert_sync::<Error>();
    assert_send::<Error>();
    assert_sync::<TlsStream<TcpStream>>();
    assert_send::<TlsStream<TcpStream>>();

    fn connect_future_is_send<C, S>(c: &C, s: S)
    where
        C: TlsConnector,
        S: AsyncRead + AsyncWrite + fmt::Debug + Unpin + Send + Sync + 'static,
    {
        let f = c.connect("dom", s);
        assert_send_value(f);
    }

    fn accept_future_is_send<A, S>(a: &A, s: S)
    where
        A: TlsAcceptor,
        S: AsyncRead + AsyncWrite + fmt::Debug + Unpin + Send + Sync + 'static,
    {
        let f = a.accept(s);
        assert_send_value(f);
    }
}

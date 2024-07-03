//! TCP connect drop in replacment with asteriods.
//!
//! ### Connect to any host with some enhanced functionalities like DNS caching and more.
//! [TCPConnect] is an stateful instance that can be used for connecting to any host as you would do
//! with [tokio::net::TcpStream::connect] but with enchanced functionalities.
//!
//! [TCPConnect] allows you to cache the DNS resolution result for a configured TTL, once cached
//! subsequent calls will pick the next IP adddress from the original list of IP addresses
//! returned, with the goal of distributing connections among all IP addresses.
//!
//! During cache miss stampede events [TCPConnect] will make sure that only one concurrent DNS
//! resolution is allowed per host, queing other calls for the same host. This would reduce
//! drastically the number of DNS resolutions.
//!
//! In the example below, a [`TCPConnect`] is [built][TCPConnect::builder] and used to
//! further connect to any host using a DNS TTL of 60 seconds:
//! ```
//!use tokio::time::Duration;
//!use tokio_comet::*;
//!
//!#[tokio::main]
//!async fn main() {
//!   let comet = TCPConnect::builder()
//!       .dns_ttl(Duration::from_secs(60))
//!       .build();
//!
//!   let mut stream = comet::connect("localhost:8080").await?;
//!
//!   stream.write_all(b"hello world!").await?;
//!
//!   Ok(())
//!}
//! ```
mod dns_cache;
mod inner;
use crate::inner::TCPConnectShared;
use std::io;
use std::sync::Arc;
use tokio::net::{TcpStream, ToSocketAddrs};
use tokio::time::Duration;
pub use dns_cache::DNSCache;

/// TCP stream connector with asteriods.
///
/// This type is internally reference-counted and can be freely cloned.
pub struct TCPConnect {
    inner: Arc<TCPConnectShared>,
}

impl TCPConnect {
    /// Create a new builder to configure the [`TCPConnect`] instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use tokio_comet::TCPConnect;
    /// use std::time::Duration;
    ///
    /// let comet = TCPConnect::builder()
    ///     .dns_ttl(Duration::from_secs(60))
    ///     .build();
    /// ```
    pub fn builder() -> TCPConnectBuilder {
        TCPConnectBuilder::default()
    }

    pub async fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<TcpStream> {
        //TODO: why we can not return just the inner future
        self.inner.connect(addr).await
    }
}

impl Clone for TCPConnect {
    fn clone(&self) -> Self {
        TCPConnect {
            inner: self.inner.clone(),
        }
    }
}

/// This builder allows you to configure the [`TCPConnect`] instance.
#[derive(Debug, Clone, Copy)]
pub struct TCPConnectBuilder {
    dns_ttl: Duration,
    dns_max_stale_time: Duration,
}

impl TCPConnectBuilder {
    /// Set the DNS TTL.
    ///
    /// By default, this is set to 60 seconds.
    pub fn dns_ttl(mut self, dns_ttl: Duration) -> TCPConnectBuilder {
        self.dns_ttl = dns_ttl;
        self
    }

    /// Set the maximum time for returning a stale DNS resolution result.
    ///
    /// After the DNS TTL (Time To Live) expires, new requests can be served
    /// stale data while a new resolution is performed in the background. Once
    /// the stale time elapses, new requests will wait until the background resolution
    /// is completed.
    ///
    /// By default, this is set to 1 second.
    pub fn dns_max_stale_time(mut self, dns_max_stale_time: Duration) -> TCPConnectBuilder {
        self.dns_max_stale_time = dns_max_stale_time;
        self
    }

    /// Build the [`TCPConnect`] instance.
    pub fn build(self) -> TCPConnect {
        let inner = Arc::new(TCPConnectShared::new(self.dns_ttl, self.dns_max_stale_time));
        TCPConnect { inner }
    }
}

impl Default for TCPConnectBuilder {
    fn default() -> Self {
        TCPConnectBuilder {
            dns_ttl: Duration::from_secs(60),
            dns_max_stale_time: Duration::from_secs(1),
        }
    }
}

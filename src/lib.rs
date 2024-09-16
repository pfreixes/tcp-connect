//! TCP connect is a ~ drop in replacment of the `std::net::TcpStream::connect` with for DNS caching.
//!
//! [TCPConnect] allows you to cache the DNS resolution result for a configured TTL, once cached
//! subsequent calls will pick a random IP adddress from the original list of IP addresses
//! returned, with the goal of distributing connections among all IP addresses.
//!
//! During cache miss stampede events [TCPConnect] will make sure that only one concurrent DNS
//! resolution is allowed per host, queing other calls for the same host. This would reduce
//! drastically the number of DNS resolutions.
//!
//! In the example below, a [`TCPConnect`] is [built][TCPConnect::builder] and used to
//! further connect to any host using a DNS TTL of 60 seconds and a maximum stale time of 1 second.
//! ```
//!use std::time::Duration;
//!use tcp_connect::*;
//!use std::io::Write;
//!
//!fn main() {
//!   let tcp_connect = TCPConnect::builder()
//!       .dns_ttl(Duration::from_secs(60))
//!       .dns_max_stale_time(Duration::from_secs(1))
//!       .build();
//!
//!   tcp_connect.connect("localhost:80");
//!}
//! ```
mod dns_cache;
mod inner;
mod resolver;
mod wait_queue;
use crate::dns_cache::InMemoryDNSCache;
use crate::inner::TCPConnectShared;
use crate::resolver::ToSocketAddrDNSResolver;
use std::io;
use std::net::TcpStream;
use std::sync::Arc;
use std::time::Duration;

#[cfg(test)]
mod test_utils;

/// TCP stream connector with DNS caching.
///
/// This type is internally reference-counted and can be freely cloned.
pub struct TCPConnect {
    inner: Arc<TCPConnectShared<InMemoryDNSCache, ToSocketAddrDNSResolver>>,
}

impl TCPConnect {
    /// Create a new builder to configure the [`TCPConnect`] instance.
    ///
    /// # Examples
    ///
    /// ```
    /// use std::time::Duration;
    /// use tcp_connect::TCPConnect;
    ///
    /// let comet = TCPConnect::builder()
    ///     .dns_ttl(Duration::from_secs(60))
    ///     .dns_max_stale_time(Duration::from_secs(1))
    ///     .build();
    /// ```
    pub fn builder() -> TCPConnectBuilder {
        TCPConnectBuilder::default()
    }

    // Connect to speciric address.
    //
    // Behind the scenes will make usage of the cached DNS resolution result.
    pub fn connect(&self, addr: &str) -> io::Result<TcpStream> {
        self.inner.connect(addr)
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
    /// stale data while a new resolution is performed in the background.
    ///
    /// By default, this is set to 1 second.
    pub fn dns_max_stale_time(mut self, dns_max_stale_time: Duration) -> TCPConnectBuilder {
        self.dns_max_stale_time = dns_max_stale_time;
        self
    }

    /// Build the [`TCPConnect`] instance.
    pub fn build(self) -> TCPConnect {
        let inner = Arc::new(TCPConnectShared::new(
            Arc::new(InMemoryDNSCache::new(self.dns_ttl, self.dns_max_stale_time)),
            Arc::new(ToSocketAddrDNSResolver::new()),
        ));
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

use crate::dns_cache::DNSCache;
use std::io;
use tokio::net::{lookup_host, TcpStream, ToSocketAddrs};
use tokio::time::Duration;

pub(crate) struct TCPConnectShared {
    dns_cache: DNSCache<String>,
}

impl TCPConnectShared {
    pub(crate) fn new(dns_ttl: Duration, dns_max_stale_time: Duration) -> Self {
        TCPConnectShared {
            dns_cache: DNSCache::new(dns_ttl, dns_max_stale_time),
        }
    }

    pub async fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<TcpStream> {
        let addrs = lookup_host(addr).await?;

        for addr in addrs {
            println!("addr {} 3", addr);
        }

        Err(io::Error::new(io::ErrorKind::InvalidInput, "mhe"))
    }
}

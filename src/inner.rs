use std::io;
use tokio::net::{ToSocketAddrs, TcpStream, lookup_host};
use tokio::time::Duration;

pub(crate) struct TCPConnectShared {
    dns_ttl: Duration,
}

impl TCPConnectShared {
    pub(crate) fn new(dns_ttl: Duration) -> Self {
        TCPConnectShared { dns_ttl }
    }

    pub async fn connect<A: ToSocketAddrs>(&self, addr: A) -> io::Result<TcpStream> {
        let addrs = lookup_host(addr).await?;

        for addr in addrs {
            println!("addr {} 3", addr);
        }

        Err(
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "mhe",
            )
        )
    }
}

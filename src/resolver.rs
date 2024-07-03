use std::io;
use std::net::{SocketAddr, ToSocketAddrs};

pub(crate) trait DNSResolver {
    fn lookup(&self, hostname: &str) -> Result<Vec<SocketAddr>, io::Error>;
}

pub(crate) struct ToSocketAddrDNSResolver;

impl ToSocketAddrDNSResolver {
    pub(crate) fn new() -> Self {
        ToSocketAddrDNSResolver {}
    }
}

impl DNSResolver for ToSocketAddrDNSResolver {
    fn lookup(&self, hostname: &str) -> Result<Vec<SocketAddr>, io::Error> {
        let addrs: Vec<SocketAddr> = (hostname, 0).to_socket_addrs()?.collect();
        Ok(addrs)
    }
}

unsafe impl Send for ToSocketAddrDNSResolver {}
unsafe impl Sync for ToSocketAddrDNSResolver {}

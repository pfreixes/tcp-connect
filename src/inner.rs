use crate::dns_cache::{CacheResult, DNSCache};
use crate::resolver::DNSResolver;
use crate::wait_queue::WaitQueue;
use rand::prelude::*;
use std::collections::HashSet;
use std::io;
use std::io::{Error, ErrorKind};
use std::net::TcpStream;
use std::net::{IpAddr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::thread::spawn;

pub(crate) struct TCPConnectShared<
    D: DNSCache + Send + Sync + 'static,
    R: DNSResolver + Send + Sync + 'static,
> {
    dns_cache: Arc<D>,
    resolver: Arc<R>,
    wait_queue: WaitQueue<D, R>,
    stale_refreshing: Arc<Mutex<HashSet<String>>>,
}

impl<D: DNSCache + Send + Sync + 'static, R: DNSResolver + Send + Sync + 'static>
    TCPConnectShared<D, R>
{
    pub(crate) fn new(dns_cache: Arc<D>, resolver: Arc<R>) -> Self {
        TCPConnectShared {
            dns_cache: dns_cache.clone(),
            resolver: resolver.clone(),
            wait_queue: WaitQueue::new(dns_cache.clone(), resolver.clone()),
            stale_refreshing: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    pub(crate) fn connect(&self, addr: &str) -> io::Result<TcpStream> {
        let (host, port) = host_port(addr)?;

        if let Ok(_) = host.parse::<IpAddr>() {
            return TcpStream::connect(addr);
        }

        match self.get_addr(addr, &mut rand::thread_rng()) {
            Ok(mut addr) => {
                addr.set_port(port);
                TcpStream::connect(addr)
            }
            Err(err) => Err(err),
        }
    }

    // Retrieves the cached value from the cache. If there is returns a random
    // address, if not will trigger a new reolution for the hostname.
    //
    // In case of having a value in the cache, if its considred stale will also
    // trigger a background resolution for refreshing the value.
    pub(self) fn get_addr<A: Rng>(
        &self,
        hostname: &str,
        rng: &mut A,
    ) -> Result<SocketAddr, io::Error> {
        let result = match self.dns_cache.get(hostname) {
            CacheResult::Hit(is_stale, addrs) => {
                if is_stale {
                    let mut stale_refreshing = self.stale_refreshing.lock().unwrap();
                    if stale_refreshing.insert(hostname.to_string()) {
                        drop(stale_refreshing);
                        self.resolve_in_background(hostname);
                    }
                }
                Ok(addrs)
            }
            CacheResult::Miss(_) => self.wait_queue.resolve_or_wait(hostname),
        };

        match result {
            Ok(addrs) => {
                let addr = *addrs.get(rng.gen::<usize>() % addrs.len()).unwrap();
                Ok(addr)
            }
            Err(err) => Err(err),
        }
    }

    // Makes a resolution of a hostname in background.
    //
    // Will update the result, if there is, of the cache. Also will
    // take care of updating the flag for allowing new background resoulutions
    // in the future when required.
    //
    // There is an expectation that the cache key must exists and not
    // have a None value.
    fn resolve_in_background(&self, hostname: &str) -> () {
        let hostname = hostname.to_string();
        let dns_cache = self.dns_cache.clone();
        let stale_refreshing = self.stale_refreshing.clone();
        let resolver = self.resolver.clone();
        //TODO Implemnet also for Tokio
        spawn(move || {
            match resolver.lookup(&hostname) {
                Ok(addrs) => {
                    dns_cache.update(&hostname, addrs);
                }
                Err(err) => {
                    // In case of an error for the backgound refresher we do not
                    // bubble up the error, either other stale events will trigger
                    // new attempts or when the TTL is expired then we will make
                    // sure that we bubble up any error to the caller.
                    // TODO: print the hostname?
                    eprintln!("error stale resolution for {hostname}: {err}");
                }
            };
            stale_refreshing.lock().unwrap().insert(hostname);
        });
    }
}

fn host_port(addr: &str) -> Result<(&str, u16), Error> {
    let (host, port_str) = addr
        .rsplit_once(":")
        .ok_or(Error::new(ErrorKind::Other, "invalid addr format"))?;
    let port: u16 = port_str
        .parse()
        .map_err(|_| Error::new(ErrorKind::Other, "invalid port"))?;
    Ok((host, port))
}

#[cfg(test)]
use crate::test_utils::tests_utils::{MockDNSCacheBuilder, MockDNSResolverBuilder};

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::mock::StepRng;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
    use std::thread::scope;
    use std::time::Duration;

    #[test]
    fn tcp_connect_shared_get_addr_return_random_addr() {
        let addr1: SocketAddr = "192.168.0.1:80".parse().unwrap();
        let addr2: SocketAddr = "192.168.0.2:80".parse().unwrap();
        let addrs_result = vec![addr1.clone(), addr2.clone()];
        let tcp_connect_shared = Arc::new(TCPConnectShared::new(
            Arc::new(
                MockDNSCacheBuilder::new()
                    .get_result(Some(addrs_result.clone()))
                    .build(),
            ),
            Arc::new(MockDNSResolverBuilder::new().build()),
        ));

        let rng = &mut StepRng::new(0, 1);

        assert_eq!(
            tcp_connect_shared.get_addr("hostname", rng).unwrap(),
            "192.168.0.1:80".parse().unwrap()
        );
        assert_eq!(
            tcp_connect_shared.get_addr("hostname", rng).unwrap(),
            "192.168.0.2:80".parse().unwrap()
        );
        assert_eq!(
            tcp_connect_shared.get_addr("hostname", rng).unwrap(),
            "192.168.0.1:80".parse().unwrap()
        );
    }

    #[test]
    fn tcp_connect_shared_get_addr_stale() {
        let addr: SocketAddr = "192.168.0.1:80".parse().unwrap();
        let addrs_result = vec![addr.clone()];
        let lookup_calls = Arc::new(AtomicU32::new(0));
        let (tx, rx): (SyncSender<bool>, Receiver<bool>) = sync_channel(1);
        let tcp_connect_shared = Arc::new(TCPConnectShared::new(
            Arc::new(
                MockDNSCacheBuilder::new()
                    .stale(true)
                    .get_result(Some(addrs_result.clone()))
                    .build(),
            ),
            Arc::new(
                MockDNSResolverBuilder::new()
                    .lookup_calls(lookup_calls.clone())
                    .sender_post_lookup(Arc::new(Mutex::new(tx)))
                    .build(),
            ),
        ));

        scope(|s| {
            for _ in 0..4 {
                let tcp_connect_shared = tcp_connect_shared.clone();
                let addr = addr.clone();
                s.spawn(move || {
                    let rng = &mut StepRng::new(0, 1);
                    let result = tcp_connect_shared.get_addr("hostname", rng);
                    assert_eq!(result.unwrap(), addr);
                });
            }
        });
        rx.recv().unwrap();
        assert_eq!(lookup_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn tcp_connect_shared_connect_invalid_addr() {
        let tcp_connect_shared = TCPConnectShared::new(
            Arc::new(MockDNSCacheBuilder::new().build()),
            Arc::new(MockDNSResolverBuilder::new().build()),
        );
        assert!(tcp_connect_shared.connect("hostnamewithoutport").is_err());
    }

    #[test]
    fn tcp_connect_shared_connect_invalid_port() {
        let tcp_connect_shared = TCPConnectShared::new(
            Arc::new(MockDNSCacheBuilder::new().build()),
            Arc::new(MockDNSResolverBuilder::new().build()),
        );
        assert!(tcp_connect_shared.connect("hostname:invalidport").is_err());
    }
}

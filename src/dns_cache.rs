//! DNS cache implementation.
use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::RwLock;
use std::time::{Duration, Instant};

pub(crate) trait DNSCache {
    fn get(&self, hostname: &str) -> CacheResult;
    fn update(&self, hostname: &str, addrs: Vec<SocketAddr>) -> ();
}

#[derive(PartialEq)]
pub(crate) enum MissReason {
    TTLExpired,
    NotFound,
}

pub(crate) type Stale = bool;

pub(crate) enum CacheResult {
    Hit(Stale, Vec<SocketAddr>),
    Miss(MissReason),
}

type InMemoryDNSCacheValue = Option<(Instant, Vec<SocketAddr>)>;

pub(crate) struct InMemoryDNSCache {
    ttl: Duration,
    max_stale: Duration,
    cache: RwLock<HashMap<String, RwLock<InMemoryDNSCacheValue>>>,
}

unsafe impl Send for InMemoryDNSCache {}
unsafe impl Sync for InMemoryDNSCache {}

impl DNSCache for InMemoryDNSCache {
    // Returns a random socket addr associated to the host if found.
    //
    // If not found will tell the reason, all of the result logic
    // is encapsulated with the `CacheResult` type.
    fn get(&self, hostname: &str) -> CacheResult {
        // Check if the key is in the cache and get the rwlock in read mode
        if let Some(value) = self.cache.read().unwrap().get(hostname) {
            // The key might be still None if there is no yet a resolution
            // made, which could happen the first time that the key is added
            // and it gets resolved, both things are done in different places
            // of the code.
            let value = value.read().unwrap();
            if let Some((inserted_time, addrs)) = value.as_ref() {
                // There is a key and its NOT None, we need to check if the
                // the last resolution is expired or not. We will also consider
                // the stale graceperiod as none expired which will be used by the
                // caller for triggering a resolution in background for refreshing
                // the value as soon as possoble.
                let now = Instant::now();
                if now < *inserted_time + self.ttl {
                    return CacheResult::Hit(false, addrs.clone());
                } else if now < *inserted_time + self.ttl + self.max_stale {
                    return CacheResult::Hit(true, addrs.clone());
                }

                return CacheResult::Miss(MissReason::TTLExpired);
            }
        }
        self.create_key(hostname);
        CacheResult::Miss(MissReason::NotFound)
    }

    // Update a DNS cache entry with a new set of socket address.
    //
    // There is an expectation that the key must exist, either
    // with Some or None value.
    // TODO: add the entry here if it does not exist
    fn update(&self, hostname: &str, addrs: Vec<SocketAddr>) -> () {
        let cache = self.cache.read().unwrap();
        let value = cache.get(hostname).unwrap();
        let mut guard = value.write().unwrap();
        *guard = Some((Instant::now(), addrs));
    }
}

impl InMemoryDNSCache {
    pub(crate) fn new(ttl: Duration, max_stale: Duration) -> Self {
        InMemoryDNSCache {
            ttl: ttl,
            max_stale: max_stale,
            cache: HashMap::new().into(),
        }
    }

    // Create the DNS cache entry for the hostname.
    fn create_key(&self, hostname: &str) -> () {
        let mut cache = self.cache.write().unwrap();
        cache
            .entry(hostname.to_string())
            .or_insert(RwLock::new(None));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::sleep;
    use std::vec;

    #[test]
    fn dns_cache_miss_and_hit() {
        let cache = InMemoryDNSCache::new(Duration::from_secs(10), Duration::from_secs(10));

        let result = cache.get("foo");
        assert!(
            matches!(result, CacheResult::Miss(MissReason::NotFound)),
            "Not a miss"
        );

        cache.update("foo", vec!["192.168.0.1:80".parse().unwrap()]);
        let result = cache.get("foo");
        match result {
            CacheResult::Hit(false, addrs) => {
                assert_eq!(addrs, vec!["192.168.0.1:80".parse().unwrap()])
            }
            _ => {
                panic!(
                    "Test failed: Expected CacheResult::Hit(false, addrs), but got something else."
                );
            }
        }
    }

    #[test]
    fn dns_cache_ttl() {
        let cache = InMemoryDNSCache::new(Duration::from_millis(1), Duration::from_millis(1));
        cache.get("foo");
        cache.update("foo", vec!["192.168.0.1:80".parse().unwrap()]);

        // will make the cache value expired
        sleep(Duration::from_millis(100));

        let result = cache.get("foo");
        assert!(
            matches!(result, CacheResult::Miss(MissReason::TTLExpired)),
            "Not TTLExpired"
        );
    }
}

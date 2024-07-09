//! DNS cache implementation.
//!
//! This cache is designed for avoiding as much as possible contention between different
//! hostname resolutions and also between same hostname resolutions.
use rand::prelude::*;
use std::collections::HashMap;
use std::hash::Hash;
use std::io::{Error, ErrorKind};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, RwLock};
use std::thread::spawn;
use std::time::Instant;
use tokio::time::Duration;

struct DNSCacheValue {
    addrs: Vec<SocketAddr>,
    inserted: Instant,
    stale_refreshing: AtomicBool,
}

struct SharedResolutionQueue {
    is_resolving: bool,
    error: Option<std::io::Error>,
}

struct ResolutionQueue {
    shared: Mutex<SharedResolutionQueue>,
    cvar: Condvar,
}

// TODO: has to be pub(crate)
pub struct DNSCache<K> {
    ttl: Duration,
    max_stale: Duration,
    cache: Arc<RwLock<HashMap<K, RwLock<Option<DNSCacheValue>>>>>,
    queues: RwLock<HashMap<K, ResolutionQueue>>,
}

impl<K> DNSCache<K>
where
    K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
{
    // TODO: has to be pub(crate)
    pub fn new(ttl: Duration, max_stale: Duration) -> Self {
        DNSCache {
            ttl: ttl,
            max_stale: max_stale,
            cache: Arc::new(HashMap::new().into()),
            queues: HashMap::new().into(),
        }
    }

    fn resolve_in_background(&self, hostname: &K) -> ()
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + 'static,
    {
        let hostname = hostname.clone();
        let cache = self.cache.clone();
        spawn(move || {
            let result = hostname.to_socket_addrs();

            let cache = cache.read().unwrap();

            let mut value = cache.get(&hostname).unwrap().write().unwrap();

            match result {
                Ok(addrs) => {
                    let value = value.as_mut().unwrap();
                    value.addrs = addrs.collect();
                    value.inserted = Instant::now();
                    value.stale_refreshing = AtomicBool::new(false);
                }
                Err(_) => {
                    // In case of an error for the backgound refresher we do not
                    // bubble up the error, either other stale events will trigger
                    // new attempts or when the TTL is expired then we will make
                    // sure that we bubble up the error.
                    let value = value.as_mut().unwrap();
                    value.stale_refreshing = AtomicBool::new(false);
                }
            }
        });
    }

    // TODO: has to be pub(crate)
    pub fn lookup<A>(&self, hostname: &K, rng: &mut A) -> Result<SocketAddr, std::io::Error>
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + 'static,
        A: Rng,
    {
        let mut found = false;
        if let Some(value) = self.cache.read().unwrap().get(hostname) {
            found = true;
            if let Some(value) = value.read().unwrap().as_ref() {
                let now = Instant::now();
                if value.inserted + self.ttl > now {
                    println!("hit");
                    let socket_addr = *value
                        .addrs
                        .get(rng.gen::<usize>() % value.addrs.len())
                        .unwrap();
                    return Ok(socket_addr);
                } else if value.inserted + self.ttl + self.max_stale > now {
                    // TODO: investigate the right ordering
                    /*
                    if let Ok(true) = value.stale_refreshing.compare_exchange(false, true, Ordering::Relaxed, Ordering::Relaxed) {
                        self.resolve_in_background(hostname);
                    }
                    */
                    let socket_addr = *value
                        .addrs
                        .get(rng.gen::<usize>() % value.addrs.len())
                        .unwrap();
                    return Ok(socket_addr);
                }
            }
        }
        println!("miss");

        if !found {
            let mut cache = self.cache.write().unwrap();
            if !cache.contains_key(hostname) {
                cache.insert((*hostname).clone(), RwLock::new(None));
            }
        }

        let mut queues = self.queues.write().unwrap();
        if !queues.contains_key(hostname) {
            println!("lookup creating key!");
            let queue = ResolutionQueue {
                shared: Mutex::new(SharedResolutionQueue {
                    is_resolving: false,
                    error: None,
                }),
                cvar: Condvar::new(),
            };
            queues.insert((*hostname).clone(), queue);
        }
        drop(queues);

        let queues = self.queues.read().unwrap();

        let queue = queues.get(hostname).unwrap();

        let mut shared = queue.shared.lock().unwrap();
        if !shared.is_resolving {
            shared.is_resolving = true;
            shared.error = None;
            drop(shared);
            drop(queue);
            drop(queues);
            match hostname.to_socket_addrs() {
                Ok(addrs) => {
                    let queues = self.queues.read().unwrap();

                    let queue = queues.get(hostname).unwrap();

                    let mut shared = queue.shared.lock().unwrap();

                    let cache = self.cache.read().unwrap();

                    let mut value = cache.get(hostname).unwrap().write().unwrap();

                    let addrs: Vec<SocketAddr> = addrs.collect();
                    let socket_addr = *addrs.get(rng.gen::<usize>() % addrs.len()).unwrap();

                    *value = Some(DNSCacheValue {
                        addrs: addrs,
                        inserted: Instant::now(),
                        stale_refreshing: AtomicBool::new(false),
                    });

                    drop(value);
                    drop(cache);

                    shared.is_resolving = false;
                    queue.cvar.notify_all();
                    drop(shared);
                    drop(queue);
                    drop(queues);

                    return Ok(socket_addr);
                }
                Err(err) => {
                    // TODO: figure out how to bubble up the orignal error
                    return Err(Error::new(ErrorKind::Other, err.to_string()));
                }
            };
        } else {
            while shared.is_resolving {
                shared = queue.cvar.wait(shared).unwrap();
            }

            if let Some(err) = shared.error.as_ref() {
                // TODO: figure out how to bubble up the orignal error
                return Err(Error::new(ErrorKind::Other, err.to_string()));
            }

            let cache = self.cache.read().unwrap();

            let value = cache.get(hostname).unwrap().read().unwrap();

            if let Some(value) = value.as_ref() {
                let socket_addr = *value
                    .addrs
                    .get(rng.gen::<usize>() % value.addrs.len())
                    .unwrap();
                return Ok(socket_addr);
            } else {
                panic!("Not implemented");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::mock::StepRng;
    use std::hash::Hasher;
    use std::io::{Error, ErrorKind};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::Arc;
    use std::thread::{scope, sleep};
    use std::vec;

    #[derive(Clone)]
    struct MockDNSCacheKey {
        hostname: String,
        result: Result<Vec<SocketAddr>, String>,
        to_socket_addrs_calls: Arc<AtomicU32>,
        to_socket_addrs_time: Duration,
    }

    impl ToSocketAddrs for MockDNSCacheKey {
        type Iter = vec::IntoIter<SocketAddr>;
        fn to_socket_addrs(&self) -> Result<Self::Iter, Error> {
            self.to_socket_addrs_calls.fetch_add(1, Ordering::Relaxed);

            sleep(self.to_socket_addrs_time);

            match &self.result {
                Ok(addr) => Ok(addr.clone().into_iter()),
                Err(error) => Err(Error::new(ErrorKind::Other, error.clone())),
            }
        }
    }

    impl PartialEq for MockDNSCacheKey {
        fn eq(&self, other: &Self) -> bool {
            self.hostname == other.hostname
        }
        fn ne(&self, other: &Self) -> bool {
            self.hostname != other.hostname
        }
    }

    impl Eq for MockDNSCacheKey {}

    impl Hash for MockDNSCacheKey {
        fn hash<H: Hasher>(&self, state: &mut H) {
            self.hostname.hash(state);
        }
    }

    impl MockDNSCacheKey {
        fn default(to_socket_addrs_calls: Arc<AtomicU32>) -> MockDNSCacheKey {
            MockDNSCacheKey {
                hostname: "host".to_string(),
                result: Ok(vec![
                    "192.168.0.1:80".parse().unwrap(),
                    "192.168.0.2:80".parse().unwrap(),
                ]),
                to_socket_addrs_calls: to_socket_addrs_calls,
                to_socket_addrs_time: Duration::from_secs(0),
            }
        }
    }

    #[test]
    fn dns_cache_miss_and_hit() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = MockDNSCacheKey::default(to_socket_addrs_calls.clone());
        let cache = DNSCache::new(Duration::from_secs(10), Duration::from_secs(10));
        let rng = &mut StepRng::new(0, 1);

        cache.lookup(&mock, rng).unwrap();
        cache.lookup(&mock, rng).unwrap();

        assert_eq!(to_socket_addrs_calls.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn dns_cache_ttl() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = MockDNSCacheKey::default(to_socket_addrs_calls.clone());
        let cache = DNSCache::new(Duration::from_millis(1), Duration::from_millis(1));
        let rng = &mut StepRng::new(0, 1);

        cache.lookup(&mock, rng).unwrap();
        assert_eq!(to_socket_addrs_calls.load(Ordering::Relaxed), 1);

        // will make the cache value expired
        sleep(Duration::from_millis(100));

        cache.lookup(&mock, rng).unwrap();
        assert_eq!(to_socket_addrs_calls.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn dns_cache_pick_next_addr() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = MockDNSCacheKey::default(to_socket_addrs_calls.clone());
        let cache = DNSCache::new(Duration::from_secs(10), Duration::from_secs(10));
        let rng = &mut StepRng::new(0, 1);

        assert_eq!(
            cache.lookup(&mock, rng).unwrap(),
            "192.168.0.1:80".parse().unwrap()
        );
        assert_eq!(
            cache.lookup(&mock, rng).unwrap(),
            "192.168.0.2:80".parse().unwrap()
        );
        assert_eq!(
            cache.lookup(&mock, rng).unwrap(),
            "192.168.0.1:80".parse().unwrap()
        );
        assert_eq!(
            cache.lookup(&mock, rng).unwrap(),
            "192.168.0.2:80".parse().unwrap()
        );
    }

    #[test]
    fn dns_cache_bubbles_up_to_socket_addr_errors() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = MockDNSCacheKey {
            hostname: "host".to_string(),
            result: Err("this is an error".to_string()),
            to_socket_addrs_calls: to_socket_addrs_calls.clone(),
            to_socket_addrs_time: Duration::from_secs(0),
        };
        let cache = DNSCache::new(Duration::from_secs(10), Duration::from_secs(10));
        let rng = &mut StepRng::new(0, 1);

        assert!(cache.lookup(&mock, rng).is_err())
    }

    #[test]
    fn dns_cache_avoid_cache_stampede() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockDNSCacheKey {
            hostname: "host".to_string(),
            result: Ok(vec![
                "192.168.0.1:80".parse().unwrap(),
                "192.168.0.2:80".parse().unwrap(),
            ]),
            to_socket_addrs_calls: to_socket_addrs_calls.clone(),
            to_socket_addrs_time: Duration::from_millis(100),
        });
        let cache = Arc::new(DNSCache::new(
            Duration::from_secs(10),
            Duration::from_secs(10),
        ));

        scope(|s| {
            s.spawn({
                let cache1 = cache.clone();
                let mock1 = mock.clone();
                move || cache1.lookup(&*mock1, &mut rand::thread_rng())
            });
            s.spawn({
                let cache2 = cache.clone();
                let mock2 = mock.clone();
                move || cache2.lookup(&*mock2, &mut rand::thread_rng())
            });
        });

        assert_eq!(to_socket_addrs_calls.load(Ordering::Relaxed), 1);
    }
}

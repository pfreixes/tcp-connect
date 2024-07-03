//! DNS cache implementation.
use rand::prelude::*;
use std::collections::HashMap;
use std::fmt::Display;
use std::hash::Hash;
use std::io::{Error, ErrorKind};
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, RwLock};
use std::thread::spawn;
use std::time::Instant;
use tokio::time::Duration;

// TODO: has to be pub(crate)
pub struct DNSCache<K> {
    inner: Arc<SharedDNSCache<K>>,
    stale_refreshing: Arc<AtomicBool>,
}

impl<K> DNSCache<K> {
    // TODO: has to be pub(crate)
    pub fn new(ttl: Duration, max_stale: Duration) -> Self {
        DNSCache {
            inner: Arc::new(SharedDNSCache::new(ttl, max_stale)),
            stale_refreshing: Arc::new(AtomicBool::new(false)),
        }
    }

    // Makes a lookup of a key and returns a random addr or an error.
    //
    // Behing the scenes this function will take care of performing the corresponding
    // resolutions if addrs are not present for the host or they are considered expired.
    // TODO: has to be pub(crate)
    pub fn lookup<A>(&self, hostname: &K, rng: &mut A) -> Result<SocketAddr, std::io::Error>
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + Display + 'static,
        A: Rng,
    {
        match self.inner.get_addr(hostname, rng) {
            CacheResult::Hit(is_stale, addr) => {
                if is_stale {
                    if let Ok(false) = self.stale_refreshing.compare_exchange(
                        false,
                        true,
                        Ordering::Acquire,
                        Ordering::Relaxed,
                    ) {
                        self.resolve_in_background(hostname);
                    }
                }
                return Ok(addr);
            }
            CacheResult::Miss(reason) => {
                if reason == MissReason::NotFound {
                    self.inner.create_key(hostname);
                }
                self.inner.resolve_or_wait(hostname, rng)
            }
        }
    }

    // Makes a resoulution of a hostname in background.
    //
    // Will update the result, if there is, of the cache. Also will
    // take care of updating the flag for allowing new background resoulutions
    // in the future when required.
    //
    // There is an expectation that the cache key must exists and not
    // have a None value.
    fn resolve_in_background(&self, hostname: &K) -> ()
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + Display + 'static,
    {
        let hostname = hostname.clone();
        let cache = self.inner.clone();
        let stale_refreshing = self.stale_refreshing.clone();
        spawn(move || {
            match hostname.to_socket_addrs() {
                Ok(addrs) => {
                    cache.insert(&hostname, addrs.collect());
                }
                Err(err) => {
                    // In case of an error for the backgound refresher we do not
                    // bubble up the error, either other stale events will trigger
                    // new attempts or when the TTL is expired then we will make
                    // sure that we bubble up any error to the caller.
                    eprintln!("error stale resolution for {hostname}: {err}");
                }
            };
            stale_refreshing.store(false, Ordering::Release);
        });
    }
}

#[derive(PartialEq)]
enum MissReason {
    TTLExpired,
    NotFound,
}

type Stale = bool;

enum CacheResult {
    Hit(Stale, SocketAddr),
    Miss(MissReason),
}

type DNSCacheValue = Option<(Instant, Vec<SocketAddr>)>;

struct SharedDNSCache<K> {
    ttl: Duration,
    max_stale: Duration,
    cache: RwLock<
        HashMap<
            K,
            (
                Arc<Mutex<(bool, Option<std::io::Error>)>>,
                Arc<Condvar>,
                RwLock<DNSCacheValue>,
            ),
        >,
    >,
}

impl<K> SharedDNSCache<K> {
    // TODO: has to be pub(crate)
    pub fn new(ttl: Duration, max_stale: Duration) -> Self {
        SharedDNSCache {
            ttl: ttl,
            max_stale: max_stale,
            cache: HashMap::new().into(),
        }
    }

    // Returns a random socket addr associated to the host if found.
    //
    // If not found will tell the reason, all of the result logic
    // is encapsulated with the `CacheResult` type.
    //
    // When results are stale, a new background resolution might be triggered,
    // if and only if there is no other one already performing such resolution.
    fn get_addr<A>(&self, hostname: &K, rng: &mut A) -> CacheResult
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
        A: Rng,
    {
        // Check if the key is in the cache and get the rwlock in read mode
        if let Some((_, _, value)) = self.cache.read().unwrap().get(hostname) {
            // The key might be still None if there is no yet a resolution
            // made, which could happen the first time that the key is added
            // and it gets resolved, both things are done in different places
            // of the code.
            let value = value.read().unwrap();
            if let Some((inserted_time, addrs)) = value.as_ref() {
                // There is a key and its NOT None, we need to check if the
                // the last resolution is expired or not. We will also consider
                // the stale graceperiod as none expired but triggering a resolution
                // in background for refreshing the value as soon as possoble.
                let now = Instant::now();
                if now < *inserted_time + self.ttl {
                    let addr = *addrs.get(rng.gen::<usize>() % addrs.len()).unwrap();
                    return CacheResult::Hit(false, addr);
                } else if now < *inserted_time + self.ttl + self.max_stale {
                    let addr = *addrs.get(rng.gen::<usize>() % addrs.len()).unwrap();
                    return CacheResult::Hit(true, addr);
                }
            }

            // Either the key was not yet associated to any resolution
            // or the value that we found was expired, in both cases
            // we return a simple TTLExpired which will either trigger
            // a new reoluton or if there is already one in progres will
            // make wait the caller until the result is available.
            return CacheResult::Miss(MissReason::TTLExpired);
        }

        CacheResult::Miss(MissReason::NotFound)
    }

    // Create the key associated to the dictionary with the minimum values.
    fn create_key(&self, hostname: &K) -> ()
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
    {
        let mut cache = self.cache.write().unwrap();
        cache.entry(hostname.clone()).or_insert((
            Arc::new(Mutex::new((false, None))),
            Arc::new(Condvar::new()),
            RwLock::new(None),
        ));
    }

    // Resolves or waits for an ongoing resolution.
    //
    // Ths function is used when an addr couldn't be found by the `lookup` method,
    // or it was considered expired. This function will either start a new resolution
    // or if there is one already in progress will just wait for the result returned.
    //
    // This function can return an error if the resolution that was performed by the lead
    // returned an error instead of a set of addrs.
    //
    // The resolved value, if there is, will be stored in the cache.
    fn resolve_or_wait<A>(&self, hostname: &K, rng: &mut A) -> Result<SocketAddr, std::io::Error>
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
        A: Rng,
    {
        let cache = self.cache.read().unwrap();
        let (condvar_mutex, condvar, _) = cache.get(hostname).unwrap();

        // we clone as soon as possible the required types
        // for running the whole coreography required for implementing a
        // waiting queue, and then we just release the lock to the cache.
        let condvar_mutex = condvar_mutex.clone();
        let condvar = condvar.clone();
        drop(cache);

        let mut condvar_guard = condvar_mutex.lock().unwrap();
        match condvar_guard.0 {
            true => self.wait_queue(condvar_guard, condvar, hostname, rng),
            false => {
                // Some data race could occur during an event were callers got
                // an expired value, got stuck trying to get the exclusive lock of
                // the queue while was locked by the current lead that would
                // waken up *only* the current waiters. When the caller got the exclusive
                // lock will perceive that there is no one taking the lead for resolving while
                // there was a new value in the cache.
                // For avoiding having a new and useless resolution we check the cache.
                if let CacheResult::Hit(_, addr) = self.get_addr(hostname, rng) {
                    return Ok(addr);
                }
                self.resolve_and_wakeup_queue(condvar_guard, hostname, rng)
            }
        }
    }

    // Will wait until the lead resolution finishes, either
    // with a new result into the cache or with an error.
    fn wait_queue<A>(
        &self,
        mut condvar_guard: MutexGuard<(bool, Option<Error>)>,
        condvar: Arc<Condvar>,
        hostname: &K,
        rng: &mut A,
    ) -> Result<SocketAddr, std::io::Error>
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
        A: Rng,
    {
        loop {
            condvar_guard = condvar.wait(condvar_guard).unwrap();
            if !condvar_guard.0 {
                if let Some(err) = condvar_guard.1.as_ref() {
                    return Err(Error::new(ErrorKind::Other, err.to_string()));
                }
                break;
            }
        }

        if let CacheResult::Hit(_, addr) = self.get_addr(hostname, rng) {
            return Ok(addr);
        } else {
            panic!("TODO")
        }
    }

    // Will resolve and will wake up the waiters once the resolution
    // is finished.
    fn resolve_and_wakeup_queue<A>(
        &self,
        mut condvar_guard: MutexGuard<(bool, Option<Error>)>,
        hostname: &K,
        rng: &mut A,
    ) -> Result<SocketAddr, std::io::Error>
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
        A: Rng,
    {
        // update the status and drop the lock
        condvar_guard.0 = true;
        condvar_guard.1 = None;
        drop(condvar_guard);

        let value = match hostname.to_socket_addrs() {
            Ok(addrs) => {
                let addrs: Vec<SocketAddr> = addrs.collect();
                let addr = *addrs.get(rng.gen::<usize>() % addrs.len()).unwrap();
                self.insert(hostname, addrs);
                Ok(addr)
            }
            Err(err) => Err(Error::new(err.kind(), err.to_string())),
        };

        // wakeup the waiters
        let cache = self.cache.read().unwrap();
        let (condvar_mutex, condvar, _) = cache.get(hostname).unwrap();
        let mut condvar_guard = condvar_mutex.lock().unwrap();
        condvar_guard.0 = false;
        condvar_guard.1 = match value.as_ref() {
            Err(err) => Some(Error::new(err.kind(), err.to_string())),
            _ => None,
        };
        condvar.notify_all();
        return value;
    }

    // Inserts a set of socket address into the cache.
    //
    // Will take care of locking all required locks and either
    // mutate the existing key or add a new one in case it has
    // None value assigned.
    //
    // There is an expectation that the key must exist, either
    // with Some or None value.
    fn insert(&self, hostname: &K, addrs: Vec<SocketAddr>) -> ()
    where
        K: Eq + PartialEq + Hash + Clone + ToSocketAddrs + Send + Sync + 'static,
    {
        let cache = self.cache.read().unwrap();
        let (_, _, ref value) = cache.get(hostname).unwrap();
        let mut guard = value.write().unwrap();
        *guard = Some((Instant::now(), addrs));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rand::rngs::mock::StepRng;
    use std::fmt;
    use std::hash::Hasher;
    use std::io::{Error, ErrorKind};
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::thread::{scope, sleep, spawn};
    use std::vec;

    #[derive(Clone)]
    struct MockDNSCacheKey {
        hostname: String,
        result: Result<Vec<SocketAddr>, String>,
        to_socket_addrs_calls: Arc<AtomicU32>,
        to_socket_addrs_time: Duration,
        receiver_pre_lookup: Arc<Option<Mutex<Receiver<bool>>>>,
        sender_post_lookup: Arc<Option<Mutex<SyncSender<bool>>>>,
    }

    impl fmt::Display for MockDNSCacheKey {
        fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
            write!(f, "{}", self.hostname)
        }
    }

    impl ToSocketAddrs for MockDNSCacheKey {
        type Iter = vec::IntoIter<SocketAddr>;
        fn to_socket_addrs(&self) -> Result<Self::Iter, Error> {
            if let Some(receiver_pre_lookup) = self.receiver_pre_lookup.as_ref() {
                receiver_pre_lookup.lock().unwrap().recv().unwrap();
            }

            sleep(self.to_socket_addrs_time);
            self.to_socket_addrs_calls.fetch_add(1, Ordering::Relaxed);

            if let Some(sender_post_lookup) = self.sender_post_lookup.as_ref() {
                sender_post_lookup.lock().unwrap().send(true).unwrap();
            }

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
                receiver_pre_lookup: Arc::new(None),
                sender_post_lookup: Arc::new(None),
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
            receiver_pre_lookup: Arc::new(None),
            sender_post_lookup: Arc::new(None),
        };
        let cache = DNSCache::new(Duration::from_secs(10), Duration::from_secs(10));
        let rng = &mut StepRng::new(0, 1);

        assert!(cache.lookup(&mock, rng).is_err())
    }

    #[test]
    fn dns_cache_wait_queue() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockDNSCacheKey {
            hostname: "host".to_string(),
            result: Ok(vec![
                "192.168.0.1:80".parse().unwrap(),
                "192.168.0.2:80".parse().unwrap(),
            ]),
            to_socket_addrs_calls: to_socket_addrs_calls.clone(),
            to_socket_addrs_time: Duration::from_millis(100),
            receiver_pre_lookup: Arc::new(None),
            sender_post_lookup: Arc::new(None),
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

    #[test]
    fn dns_cache_wait_queue_with_error() {
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let mock = Arc::new(MockDNSCacheKey {
            hostname: "host".to_string(),
            result: Err("this is an error".to_string()),
            to_socket_addrs_calls: to_socket_addrs_calls.clone(),
            to_socket_addrs_time: Duration::from_millis(100),
            receiver_pre_lookup: Arc::new(None),
            sender_post_lookup: Arc::new(None),
        });
        let cache = Arc::new(DNSCache::new(
            Duration::from_secs(10),
            Duration::from_secs(10),
        ));

        scope(|s| {
            s.spawn({
                let cache1 = cache.clone();
                let mock1 = mock.clone();
                move || {
                    assert!(cache1.lookup(&*mock1, &mut rand::thread_rng()).is_err());
                }
            });
            s.spawn({
                let cache2 = cache.clone();
                let mock2 = mock.clone();
                move || {
                    assert!(cache2.lookup(&*mock2, &mut rand::thread_rng()).is_err());
                }
            });
        });

        assert_eq!(to_socket_addrs_calls.load(Ordering::Relaxed), 1);
    }

    // TODO: change this test!!!
    #[test]
    fn dns_cache_max_stale() {
        let (sender_pre_lookup, receiver_pre_lookup) = sync_channel(0);
        let (sender_post_lookup, receiver_post_lookup) = sync_channel(0);
        let to_socket_addrs_calls = Arc::new(AtomicU32::new(0));
        let to_socket_addrs_calls_cloned = to_socket_addrs_calls.clone();

        let handle = spawn(move || {
            let cache = DNSCache::new(Duration::from_millis(1), Duration::from_secs(1));
            let mock = MockDNSCacheKey {
                hostname: "host".to_string(),
                result: Ok(vec![
                    "192.168.0.1:80".parse().unwrap(),
                    "192.168.0.2:80".parse().unwrap(),
                ]),
                to_socket_addrs_calls: to_socket_addrs_calls_cloned,
                to_socket_addrs_time: Duration::from_millis(0),
                receiver_pre_lookup: Arc::new(Some(Mutex::new(receiver_pre_lookup))),
                sender_post_lookup: Arc::new(Some(Mutex::new(sender_post_lookup))),
            };
            let rng = &mut StepRng::new(0, 1);

            cache.lookup(&mock, rng).unwrap();

            // will make the cache value stale
            sleep(Duration::from_millis(100));

            for n in 0..10 {
                cache.lookup(&mock, rng).unwrap();
            }
        });

        // wait and unblock for the first lookup
        sender_pre_lookup.send(true).unwrap();
        receiver_post_lookup.recv().unwrap();

        // wait and unblock the background lookup
        sender_pre_lookup.send(true).unwrap();
        receiver_post_lookup.recv().unwrap();

        // wait for the thread
        handle.join().unwrap();

        assert_eq!(to_socket_addrs_calls.load(Ordering::Relaxed), 2);
    }
}

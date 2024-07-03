#[cfg(test)]
pub(crate) mod tests_utils {
    use crate::dns_cache::{CacheResult, DNSCache, MissReason};
    use crate::resolver::DNSResolver;
    use std::io;
    use std::io::{Error, ErrorKind};
    use std::net::SocketAddr;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::mpsc::{Receiver, SyncSender};
    use std::sync::{Arc, Mutex};
    use std::thread::sleep;
    use std::time::Duration;
    use std::vec;

    pub(crate) struct MockDNSResolver {
        pub(crate) result: Result<Vec<SocketAddr>, String>,
        pub(crate) lookup_calls: Arc<AtomicU32>,
        pub(crate) lookup_time: Duration,
        pub(crate) receiver_pre_lookup: Option<Arc<Mutex<Receiver<bool>>>>,
        pub(crate) sender_post_lookup: Option<Arc<Mutex<SyncSender<bool>>>>,
    }

    pub(crate) struct MockDNSResolverBuilder {
        result: Option<Result<Vec<SocketAddr>, String>>,
        lookup_calls: Option<Arc<AtomicU32>>,
        lookup_time: Option<Duration>,
        receiver_pre_lookup: Option<Arc<Mutex<Receiver<bool>>>>,
        sender_post_lookup: Option<Arc<Mutex<SyncSender<bool>>>>,
    }

    impl MockDNSResolverBuilder {
        pub(crate) fn new() -> Self {
            Self {
                result: None,
                lookup_calls: None,
                lookup_time: None,
                receiver_pre_lookup: None,
                sender_post_lookup: None,
            }
        }

        pub(crate) fn result(mut self, result: Result<Vec<SocketAddr>, String>) -> Self {
            self.result = Some(result);
            self
        }

        pub(crate) fn lookup_calls(mut self, lookup_calls: Arc<AtomicU32>) -> Self {
            self.lookup_calls = Some(lookup_calls);
            self
        }

        pub(crate) fn lookup_time(mut self, lookup_time: Duration) -> Self {
            self.lookup_time = Some(lookup_time);
            self
        }

        pub(crate) fn receiver_pre_lookup(
            mut self,
            receiver_pre_lookup: Arc<Mutex<Receiver<bool>>>,
        ) -> Self {
            self.receiver_pre_lookup = Some(receiver_pre_lookup);
            self
        }

        pub(crate) fn sender_post_lookup(
            mut self,
            sender_post_lookup: Arc<Mutex<SyncSender<bool>>>,
        ) -> Self {
            self.sender_post_lookup = Some(sender_post_lookup);
            self
        }

        pub(crate) fn build(self) -> MockDNSResolver {
            MockDNSResolver {
                result: self.result.unwrap_or_else(|| Ok(vec![])),
                lookup_calls: self
                    .lookup_calls
                    .unwrap_or_else(|| Arc::new(AtomicU32::new(0))),
                lookup_time: self.lookup_time.unwrap_or_else(|| Duration::from_secs(0)),
                receiver_pre_lookup: self.receiver_pre_lookup.or_else(|| None),
                sender_post_lookup: self.sender_post_lookup.or_else(|| None),
            }
        }
    }

    impl DNSResolver for MockDNSResolver {
        fn lookup(&self, _: &str) -> Result<Vec<SocketAddr>, io::Error> {
            if let Some(receiver_pre_lookup) = self.receiver_pre_lookup.as_ref() {
                receiver_pre_lookup.lock().unwrap().recv().unwrap();
            }

            sleep(self.lookup_time);
            self.lookup_calls.fetch_add(1, Ordering::Relaxed);

            if let Some(sender_post_lookup) = self.sender_post_lookup.as_ref() {
                sender_post_lookup.lock().unwrap().send(true).unwrap();
            }

            match &self.result {
                Ok(addr) => Ok(addr.clone()),
                Err(err) => Err(Error::new(ErrorKind::Other, err.clone())),
            }
        }
    }

    pub(crate) struct MockDNSCache {
        pub(crate) stale: bool,
        pub(crate) get_result: Option<Vec<SocketAddr>>,
        pub(crate) get_calls: Arc<AtomicU32>,
        pub(crate) update_calls: Arc<AtomicU32>,
    }

    pub(crate) struct MockDNSCacheBuilder {
        stale: Option<bool>,
        get_result: Option<Option<Vec<SocketAddr>>>,
        get_calls: Option<Arc<AtomicU32>>,
        update_calls: Option<Arc<AtomicU32>>,
    }

    impl MockDNSCacheBuilder {
        pub(crate) fn new() -> Self {
            Self {
                stale: None,
                get_result: None,
                get_calls: None,
                update_calls: None,
            }
        }

        pub(crate) fn stale(mut self, stale: bool) -> Self {
            self.stale = Some(stale);
            self
        }

        pub(crate) fn get_result(mut self, get_result: Option<Vec<SocketAddr>>) -> Self {
            self.get_result = Some(get_result);
            self
        }

        pub(crate) fn get_calls(mut self, get_calls: Arc<AtomicU32>) -> Self {
            self.get_calls = Some(get_calls);
            self
        }

        pub(crate) fn update_calls(mut self, update_calls: Arc<AtomicU32>) -> Self {
            self.update_calls = Some(update_calls);
            self
        }

        pub(crate) fn build(self) -> MockDNSCache {
            MockDNSCache {
                stale: self.stale.unwrap_or(false),
                get_result: self.get_result.unwrap_or(None),
                get_calls: self
                    .get_calls
                    .unwrap_or_else(|| Arc::new(AtomicU32::new(0))),
                update_calls: self
                    .update_calls
                    .unwrap_or_else(|| Arc::new(AtomicU32::new(0))),
            }
        }
    }

    impl DNSCache for MockDNSCache {
        fn get(&self, _: &str) -> CacheResult {
            self.get_calls.fetch_add(1, Ordering::Relaxed);
            match self.get_result.as_ref() {
                Some(addrs) => CacheResult::Hit(self.stale, addrs.clone()),
                None => CacheResult::Miss(MissReason::NotFound),
            }
        }

        fn update(&self, _: &str, _: Vec<SocketAddr>) -> () {
            self.update_calls.fetch_add(1, Ordering::Relaxed);
        }
    }
}

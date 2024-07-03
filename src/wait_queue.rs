use crate::dns_cache::DNSCache;
use crate::resolver::DNSResolver;
use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::io::Error;
use std::net::SocketAddr;
use std::sync::{Arc, Condvar, Mutex};

enum ResolvingState {
    InProgress,
    Finished(Arc<Result<Vec<SocketAddr>, Error>>),
}

struct WaitQueueItem {
    resolving_status: Mutex<ResolvingState>,
    condvar: Condvar,
}

pub(crate) struct WaitQueue<D: DNSCache, R: DNSResolver> {
    dns_cache: Arc<D>,
    resolver: Arc<R>,
    queues: Mutex<HashMap<String, Arc<WaitQueueItem>>>,
}

unsafe impl<D: DNSCache, R: DNSResolver + Send + Sync + 'static> Send for WaitQueue<D, R> {}
unsafe impl<D: DNSCache, R: DNSResolver + Send + Sync + 'static> Sync for WaitQueue<D, R> {}

impl<D: DNSCache, R: DNSResolver + Send + Sync + 'static> WaitQueue<D, R> {
    pub(crate) fn new(dns_cache: Arc<D>, resolver: Arc<R>) -> Self {
        WaitQueue {
            dns_cache: dns_cache,
            resolver: resolver,
            queues: HashMap::new().into(),
        }
    }

    // Resolves or waits for an ongoing resolution.
    //
    // Ths function is used when an addr couldn't be found using the DNS Cache,
    // or the value was considered expired. This function will either start a new resolution
    // or if there is one already in progress will just wait for the result returned.
    //
    // This function can return an error if the resolution that was performed by the leader
    // returned an error instead of a set of addrs.
    //
    // The resolved value, if there is, will be stored in the cache.
    pub(crate) fn resolve_or_wait(
        &self,
        hostname: &str,
    ) -> Result<Vec<SocketAddr>, std::io::Error> {
        let mut queues = self.queues.lock().unwrap();

        let result = match queues.entry(hostname.to_string()) {
            Entry::Vacant(vacant_entry) => {
                let queue = Arc::new(WaitQueueItem {
                    resolving_status: Mutex::new(ResolvingState::InProgress),
                    condvar: Condvar::new(),
                });
                vacant_entry.insert(queue.clone());

                drop(queues);
                let result = self.resolve_and_wakeup_waiters(hostname, queue);

                // We update the cache with the result
                if let Ok(ref addrs) = *result {
                    self.dns_cache.update(hostname, addrs.clone());
                }

                // And then we remove the queue. The Arc<WaitQueueItem> associated to the current
                // queue that is gonna be destroyed will eventually dropped when no more waiters
                // are referencing to it.
                let mut queues = self.queues.lock().unwrap();
                queues.remove(hostname);

                result
            }
            Entry::Occupied(occupied_entry) => {
                let queue = occupied_entry.get().clone();
                drop(queues);
                self.wait_queue(queue)
            }
        };

        match result.as_ref() {
            Ok(addrs) => Ok(addrs.clone()),
            Err(err) => Err(Error::new(err.kind(), err.to_string())),
        }
    }

    // Will wait until the lead resolution finishes, either
    // with a new result into the cache or with an error.
    fn wait_queue(
        &self,
        queue: Arc<WaitQueueItem>,
    ) -> Arc<Result<Vec<SocketAddr>, std::io::Error>> {
        // wait until the lead finished
        let mut resolving_status = queue.resolving_status.lock().unwrap();
        while matches!(*resolving_status, ResolvingState::InProgress) {
            resolving_status = queue.condvar.wait(resolving_status).unwrap();
        }

        match *resolving_status {
            ResolvingState::Finished(ref result) => result.clone(),
            ResolvingState::InProgress => panic!("should not be reached"),
        }
    }

    // Will resolve and will wake up the waiters once the resolution has finised.
    fn resolve_and_wakeup_waiters(
        &self,
        hostname: &str,
        queue: Arc<WaitQueueItem>,
    ) -> Arc<Result<Vec<SocketAddr>, std::io::Error>> {
        let result = Arc::new(self.resolver.lookup(hostname));

        // Update the resolution state and wakeup the waiters
        let mut resolving_status = queue.resolving_status.lock().unwrap();
        *resolving_status = ResolvingState::Finished(result.clone());
        queue.condvar.notify_all();
        return result;
    }
}

#[cfg(test)]
use crate::test_utils::tests_utils::{MockDNSCacheBuilder, MockDNSResolverBuilder};

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::thread::scope;
    use std::time::Duration;

    macro_rules! test_wait_queue_resolve_or_wait {
        ($($name:ident: $value:expr,)*) => {
        $(
            #[test]
            fn $name() {
                let expected_result = $value;
		let lookup_calls = Arc::new(AtomicU32::new(0));
                let dns_resolver = Arc::new(MockDNSResolverBuilder::new()
                    .result(expected_result.clone())
                    .lookup_time(Duration::from_millis(100))
                    .lookup_calls(lookup_calls.clone())
                    .build()
                );
		let get_calls = Arc::new(AtomicU32::new(0));
		let update_calls = Arc::new(AtomicU32::new(0));
		let dns_cache = Arc::new(MockDNSCacheBuilder::new()
		    .get_calls(get_calls.clone())
		    .update_calls(update_calls.clone())
                    .build()
		);
		let wait_queue = Arc::new(WaitQueue::new(dns_cache, dns_resolver));

		scope(|s| {
		    s.spawn({
			let wait_queue = wait_queue.clone();
                        let expected_result = expected_result.clone();
			move || {
			    let result = wait_queue.resolve_or_wait("foo");
                            match expected_result.as_ref() {
                                Ok(addrs) => {
			            assert_eq!(result.unwrap(), *addrs);
                                },
                                Err(_) => {
			            assert!(result.is_err());
                                },
                            }
		    }});
		    s.spawn({
			let wait_queue = wait_queue.clone();
                        let expected_result = expected_result.clone();
			move || {
			    let result = wait_queue.resolve_or_wait("foo");
                            match expected_result.as_ref() {
                                Ok(addrs) => {
			            assert_eq!(result.unwrap(), *addrs);
                                },
                                Err(_) => {
			            assert!(result.is_err());
                                },
                            }
		    }});
		});

                assert_eq!(lookup_calls.load(Ordering::Relaxed), 1);

                match expected_result.as_ref() {
                    Ok(_) => {
                        assert_eq!(update_calls.load(Ordering::Relaxed), 1);
                    },
                    Err(_) => (),
                }
            }
        )*
        }
    }

    test_wait_queue_resolve_or_wait! {
        test_wait_queue_resolve_or_wait_resolve_ok: Ok(vec!["192.168.0.1:80".parse().unwrap()]),
        test_wait_queue_resolve_or_wait_resolve_error: Err("error message".to_string()),
    }
}

use rand::prelude::*;
use tcp_connect::DNSCache;
use tokio::time::Duration;
use std::thread::spawn;
use std::sync::Arc;

#[test]
fn test_tcp_connect() {
    let cache = Arc::new(DNSCache::new(Duration::from_secs(10), Duration::from_secs(10)));

    for n in 1..10 {
        let cache = cache.clone();
        spawn(move || {
            println!("{:?}", cache.lookup(&"google.com:80".to_string(), &mut rand::thread_rng()));
        }).join();
    }
}

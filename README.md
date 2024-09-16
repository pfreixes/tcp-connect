TCP connect is a ~ drop in replacment of the `std::net::TcpStream::connect` with DNS caching.

`TCPConnect` allows you to cache the DNS resolution result for a configured TTL, once cached
subsequent calls will pick a random IP adddress from the original list of IP addresses
returned, with the goal of distributing connections among all IP addresses.

During cache miss stampede events `TCPConnect` will make sure that only one concurrent DNS
resolution is allowed per host, queing other calls for the same host. This would reduce
drastically the number of DNS resolutions.

In the example below, a `TCPConnect` is built and used to
further connect to any host using a DNS TTL of 60 seconds and a maximum stale time of 1 second.

```rust
use std::time::Duration;
use tcp_connect::*;
use std::io::Write;

fn main() {
   let tcp_connect = TCPConnect::builder()
       .dns_ttl(Duration::from_secs(60))
       .dns_max_stale_time(Duration::from_secs(1))
       .build();

   tcp_connect.connect("localhost:80");
}
 ```
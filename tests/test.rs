use tokio::time::Duration;
use tcp_connect::*;

#[tokio::test]
async fn test_tcp_connect() {
    let tcp_connect = TCPConnect::builder()
        .dns_ttl(Duration::from_secs(60))
        .build();

   //let _ = comet.connect("crates.io:80").await;
   let _ = tcp_connect.connect("osblbstrg6e36.blob.core.windows.net:443").await;
}

use std::net::{SocketAddr, TcpListener};
#[cfg(not(feature = "tokio"))]
use std::thread;
use tcp_connect::TCPConnect;
#[cfg(feature = "tokio")]
use tokio::net::TcpStream;
#[cfg(feature = "tokio")]
use tokio::task;

fn start_test_server() -> std::io::Result<(SocketAddr, TcpListener)> {
    let listener = TcpListener::bind("[::]:0")?;
    let addr = listener.local_addr()?;
    Ok((addr, listener))
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_tcp_connect() {
    let (addr, _listener) = start_test_server().unwrap();
    let port = addr.port();
    let tcp_connect = TCPConnect::builder().build();

    for _ in 1..10 {
        let tcp_connect = tcp_connect.clone();
        let handle: task::JoinHandle<Result<TcpStream, io::Error>> = tokio::spawn(async move {
            return tcp_connect.connect(&format!("localhost:{}", port)).await;
        });
        let result = handle.await.unwrap();
        assert!(result.is_ok())
    }
}

#[cfg(feature = "tokio")]
#[tokio::test]
async fn test_tcp_connect_using_an_ipvaddr() {
    let (addr, _listener) = start_test_server().unwrap();
    let port = addr.port();
    let tcp_connect = TCPConnect::builder().build();
    assert!(tcp_connect
        .connect(&format!("127.0.0.1:{}", port))
        .await
        .is_ok())
}

#[cfg(not(feature = "tokio"))]
#[test]
fn test_tcp_connect() {
    let (addr, _listener) = start_test_server().unwrap();
    let port = addr.port();
    let tcp_connect = TCPConnect::builder().build();

    for _ in 1..10 {
        let tcp_connect = tcp_connect.clone();
        let result = thread::spawn(move || tcp_connect.connect(&format!("localhost:{}", port)))
            .join()
            .unwrap();

        assert!(result.is_ok())
    }
}

#[cfg(not(feature = "tokio"))]
#[test]
fn test_tcp_connect_using_an_ipvaddr() {
    let (addr, _listener) = start_test_server().unwrap();
    let port = addr.port();
    let tcp_connect = TCPConnect::builder().build();
    assert!(tcp_connect.connect(&format!("127.0.0.1:{}", port)).is_ok())
}

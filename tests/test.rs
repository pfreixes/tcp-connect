use std::net::{SocketAddr, TcpListener};
use std::thread;
use tcp_connect::TCPConnect;

fn start_test_server() -> std::io::Result<(SocketAddr, TcpListener)> {
    let listener = TcpListener::bind("[::]:0")?;
    let addr = listener.local_addr()?;
    Ok((addr, listener))
}

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

fn test_tcp_connect_using_an_ipvaddr() {
    let (addr, _listener) = start_test_server().unwrap();
    let port = addr.port();
    let tcp_connect = TCPConnect::builder().build();
    assert!(tcp_connect.connect(&format!("127.0.0.1:{}", port)).is_ok())
}

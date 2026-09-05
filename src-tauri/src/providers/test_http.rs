//! Loopback wire fixtures for the actual provider implementations.
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::task::JoinHandle;
use tokio::time::{timeout, Duration};

pub(super) async fn read_request(socket: &mut TcpStream) -> Vec<u8> {
    timeout(Duration::from_secs(5), async {
        let mut bytes = Vec::new();
        loop {
            let mut chunk = [0u8; 4096];
            let count = socket.read(&mut chunk).await.unwrap();
            assert_ne!(count, 0, "client closed before completing request");
            bytes.extend_from_slice(&chunk[..count]);
            assert!(bytes.len() < 2 * 1024 * 1024, "fixture request limit");
            if let Some(boundary) = bytes.windows(4).position(|part| part == b"\r\n\r\n") {
                let headers = String::from_utf8_lossy(&bytes[..boundary]);
                let length = headers
                    .lines()
                    .find_map(|line| {
                        let (key, value) = line.split_once(':')?;
                        key.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse::<usize>().unwrap())
                    })
                    .unwrap_or(0);
                if bytes.len() >= boundary + 4 + length {
                    return bytes;
                }
            }
        }
    })
    .await
    .expect("fixture receives request")
}

pub(super) async fn respond(
    status: &str,
    body: Vec<u8>,
    extra_headers: &str,
) -> (String, JoinHandle<Vec<u8>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/x-ndjson\r\nConnection: close\r\n{extra_headers}\r\n",
        body.len(),
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = timeout(Duration::from_secs(5), listener.accept())
            .await
            .expect("fixture receives connection")
            .unwrap();
        let request = read_request(&mut socket).await;
        socket.write_all(headers.as_bytes()).await.unwrap();
        // An error or terminal frame may make the provider close early.
        let _ = socket.write_all(&body).await;
        request
    });
    (url, task)
}

pub(super) async fn hold_open(status: &str, prefix: Vec<u8>) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/x-ndjson\r\n\r\n",
        prefix.len() + 100_000,
    );
    let task = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        read_request(&mut socket).await;
        socket.write_all(headers.as_bytes()).await.unwrap();
        socket.write_all(&prefix).await.unwrap();
        std::future::pending::<()>().await;
        drop(socket);
    });
    (url, task)
}

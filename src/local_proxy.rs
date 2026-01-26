use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::proxy::ProxyConfig;

/// Local proxy server that forwards to an authenticated upstream proxy
pub struct LocalProxy {
    listener: TcpListener,
    upstream: Arc<ProxyConfig>,
}

impl LocalProxy {
    /// Start a local proxy on a random available port
    pub async fn start(upstream: ProxyConfig) -> anyhow::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        Ok(Self {
            listener,
            upstream: Arc::new(upstream),
        })
    }

    /// Get the local address (127.0.0.1:port)
    pub fn local_addr(&self) -> String {
        self.listener
            .local_addr()
            .map(|a| a.to_string())
            .unwrap_or_default()
    }

    /// Run the proxy server (spawns handler for each connection)
    pub fn run(self) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            loop {
                match self.listener.accept().await {
                    Ok((stream, _)) => {
                        let upstream = self.upstream.clone();
                        tokio::spawn(async move {
                            if let Err(e) = handle_connection(stream, &upstream).await {
                                eprintln!("Proxy connection error: {}", e);
                            }
                        });
                    }
                    Err(e) => {
                        eprintln!("Proxy accept error: {}", e);
                    }
                }
            }
        })
    }
}

/// Handle a single connection from the browser
async fn handle_connection(mut client: TcpStream, upstream: &ProxyConfig) -> anyhow::Result<()> {
    // Read request into buffer first
    let mut buf = vec![0u8; 4096];
    let n = client.read(&mut buf).await?;
    let request = String::from_utf8_lossy(&buf[..n]);

    // Parse CONNECT request
    let first_line = request.lines().next().unwrap_or("");
    let parts: Vec<&str> = first_line.split_whitespace().collect();
    if parts.len() < 2 || parts[0] != "CONNECT" {
        client
            .write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n")
            .await?;
        return Ok(());
    }
    let target = parts[1];

    // Connect to upstream proxy
    let upstream_addr = format!("{}:{}", upstream.host, upstream.port);
    let mut upstream_stream = TcpStream::connect(&upstream_addr).await?;

    // Build CONNECT request with auth
    let auth = match (&upstream.username, &upstream.password) {
        (Some(user), Some(pass)) => {
            let credentials = format!("{}:{}", user, pass);
            let encoded = base64::Engine::encode(
                &base64::engine::general_purpose::STANDARD,
                credentials.as_bytes(),
            );
            format!("Proxy-Authorization: Basic {}\r\n", encoded)
        }
        _ => String::new(),
    };

    let connect_request = format!(
        "CONNECT {} HTTP/1.1\r\nHost: {}\r\n{}Connection: keep-alive\r\n\r\n",
        target, target, auth
    );

    upstream_stream
        .write_all(connect_request.as_bytes())
        .await?;

    // Read upstream response
    let mut response_buf = vec![0u8; 4096];
    let n = upstream_stream.read(&mut response_buf).await?;
    let response = String::from_utf8_lossy(&response_buf[..n]);

    // Check for 200 OK
    if !response.starts_with("HTTP/1.1 200") && !response.starts_with("HTTP/1.0 200") {
        client
            .write_all(b"HTTP/1.1 502 Bad Gateway\r\n\r\n")
            .await?;
        return Ok(());
    }

    // Send 200 OK to browser
    client
        .write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n")
        .await?;

    // Bidirectional copy using owned halves
    let (mut client_read, mut client_write) = client.into_split();
    let (mut upstream_read, mut upstream_write) = upstream_stream.into_split();

    let client_to_upstream = async { tokio::io::copy(&mut client_read, &mut upstream_write).await };
    let upstream_to_client = async { tokio::io::copy(&mut upstream_read, &mut client_write).await };

    tokio::select! {
        _ = client_to_upstream => {}
        _ = upstream_to_client => {}
    }

    Ok(())
}

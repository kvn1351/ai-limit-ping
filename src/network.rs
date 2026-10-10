use std::net::ToSocketAddrs;
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::time::sleep;

/// Fast network check to determine if the computer has an active internet connection.
/// Tries connecting to well-known IP addresses (1.1.1.1:53, 8.8.8.8:53).
pub async fn is_online(timeout: Duration) -> bool {
    let targets = ["1.1.1.1:53", "8.8.8.8:53", "1.0.0.1:53"];
    for target in targets {
        if let Ok(mut addrs) = target.to_socket_addrs() {
            if let Some(addr) = addrs.next() {
                if tokio::time::timeout(timeout, TcpStream::connect(addr))
                    .await
                    .map(|res| res.is_ok())
                    .unwrap_or(false)
                {
                    return true;
                }
            }
        }
    }
    false
}

/// Waits for internet connectivity to be restored (e.g. after computer wake from sleep).
/// Retries with backoff up to `max_wait`.
pub async fn wait_for_connectivity(max_wait: Duration) -> bool {
    let start = tokio::time::Instant::now();
    let mut delay = Duration::from_millis(1000);

    while start.elapsed() < max_wait {
        if is_online(Duration::from_millis(1500)).await {
            return true;
        }
        sleep(delay).await;
        delay = (delay * 2).min(Duration::from_secs(5));
    }
    false
}

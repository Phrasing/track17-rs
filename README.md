# track17-rs

> Rust library, CLI, and HTTP server for tracking packages via 17track.net private API.

## Features

- **Thread-safe concurrent tracking** - Clone and share client across tasks
- **HTTP REST API server** - Production-ready microservice with Axum
- **Auto-detect carrier** - FedEx, UPS, USPS, DHL, and more
- **Batch tracking** - Track multiple packages concurrently
- **V8-powered credentials** - Embedded JavaScript runtime, no browser needed
- **Credential caching** - Shared across all requests with 1-hour TTL
- **Docker ready** - Multi-stage builds with security hardening

## Quick Start

### Docker (Recommended)

```bash
# Start the server
docker-compose up -d

# Test it
curl http://localhost:3000/health
```

### From Source

```bash
# CLI usage
cargo run -- 1234567890

# HTTP server
cargo run --bin server
```

## HTTP API

### Endpoints

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/api/metrics` | GET | Server metrics |
| `/api/track` | POST | Track single package |
| `/api/track/batch` | POST | Track multiple packages |

### Track a Package

```bash
curl -X POST http://localhost:3000/api/track \
  -H "Content-Type: application/json" \
  -d '{
    "tracking_number": "123456789012",
    "carrier_code": 0
  }'
```

Response:
```json
{
  "success": true,
  "data": {
    "tracking_number": "123456789012",
    "carrier": 100003,
    "status": "DELIVERED",
    "latest_event": {
      "time": "2024-01-15T10:30:00Z",
      "description": "Delivered",
      "location": "New York, NY"
    }
  }
}
```

## CLI Usage

```bash
# Single package (auto-detect carrier)
cargo run -- 1234567890

# Multiple packages
cargo run -- NUM1,NUM2,NUM3

# Specify carrier
cargo run -- 1234567890 fedex

# With proxy
cargo run -- 1234567890 auto "http://user:pass@proxy:8080"
```

**Supported carriers:** `auto`, `fedex`, `ups`, `usps`, `dhl`

## Library Usage

```rust
use track17_rs::{Track17Client, carriers};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Track17Client::new().await?;
    let response = client.track("1234567890", carriers::AUTO).await?;

    for shipment in &response.shipments {
        if let Some(details) = &shipment.shipment {
            if let Some(event) = &details.latest_event {
                println!("{}: {}", shipment.number, event.tracking_state());
            }
        }
    }
    Ok(())
}
```

### Concurrent Tracking

```rust
use std::sync::Arc;
use track17_rs::{Track17Client, carriers};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let client = Arc::new(Track17Client::new().await?);
    let tracking_numbers = vec!["123456789012", "234567890123"];

    let handles: Vec<_> = tracking_numbers
        .iter()
        .map(|num| {
            let client = client.clone();
            let num = (*num).to_string();
            tokio::spawn(async move {
                client.track(&num, carriers::AUTO).await
            })
        })
        .collect();

    for handle in handles {
        match handle.await? {
            Ok(response) => println!("Tracked: {}", response.shipments[0].number),
            Err(e) => eprintln!("Error: {}", e),
        }
    }
    Ok(())
}
```

### With Proxy

```rust
use track17_rs::{Track17Client, ProxyConfig, carriers};

let proxy = ProxyConfig::parse("http://user:pass@proxy:8080");
let client = Track17Client::with_proxy(proxy).await?;
```

## Docker Deployment

### Quick Start

```bash
# Start with docker-compose
docker-compose up -d

# View logs
docker-compose logs -f track17-server

# Stop
docker-compose down
```

### Manual Build

```bash
# Build
docker build -t track17-server:latest .

# Run
docker run -d \
  --name track17-server \
  -p 3000:3000 \
  -e RUST_LOG=info \
  track17-server:latest
```

### Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `PORT` | `3000` | Server listening port |
| `RUST_LOG` | `info` | Log level (`error`, `warn`, `info`, `debug`, `trace`) |
| `HOST_PORT` | `3000` | Host port mapping (docker-compose only) |

### Production

**Resource Requirements:**
- Build: 2GB+ RAM, ~5-8 minutes (first build)
- Runtime: 256-512MB RAM, 0.5-1.0 CPU core
- Image: ~300MB

**Security Features:**
- Non-root user (UID 1000)
- Minimal Debian slim base
- Automated health checks
- Graceful SIGTERM shutdown

## Architecture

### V8-Powered Credentials

Uses embedded V8 JavaScript runtime (via `deno_core`) to execute 17track's sign generation:

1. Fetch JS assets from 17track CDN (cached 1 hour)
2. Execute sign module in V8
3. Cache credentials across all instances (1-hour TTL)
4. Auto-refresh when expired

Thread-safe design allows cloning and sharing the client via `Arc<RwLock<>>` for concurrent requests.

## Troubleshooting

### Docker Build Issues

**V8 compilation fails:**
```bash
# Increase Docker memory to 4GB+
docker build --memory=4g -t track17-server:latest .
```

**Behind proxy:**
```bash
docker build \
  --build-arg HTTP_PROXY=http://proxy:8080 \
  --build-arg HTTPS_PROXY=http://proxy:8080 \
  -t track17-server:latest .
```

### Runtime Issues

**Container exits:**
```bash
docker logs track17-server
```

**Network connectivity:**
```bash
docker exec track17-server curl -I https://17track.net
```

## Examples

See [`examples/`](examples/) directory:
- [`concurrent_tracking.rs`](examples/concurrent_tracking.rs) - Concurrent tracking demo
- [`api_client.rs`](examples/api_client.rs) - HTTP client example

## License

This project is for educational purposes only. Respect 17track.net's terms of service.

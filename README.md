# track17-rs

Rust library, CLI, and HTTP server for tracking packages via 17track.net private API.

## Features

- **Thread-safe concurrent tracking** - Clone and share client across tasks
- **HTTP REST API server** - Microservice with Axum web framework
- **Auto-detect carrier** - Or specify FedEx, UPS, USPS, DHL, and more
- **Batch tracking** - Track multiple packages concurrently
- **V8-powered credentials** - Embedded JavaScript runtime, no browser needed
- **Credential caching** - Shared across all requests with 1-hour TTL

## Usage

```bash
# Single package (auto-detect carrier)
cargo run -- 1234567890

# Multiple packages
cargo run -- NUM1,NUM2,NUM3

# Specify carrier
cargo run -- 1234567890 fedex

# With proxy
cargo run -- 1234567890 auto "http://user:pass@proxy.example.com:8080"
```

### Carrier Options

- `auto` - Auto-detect (default)
- `fedex` - FedEx
- `ups` - UPS
- `usps` - USPS
- `dhl` - DHL

### Proxy Formats

```
http://user:pass@host:port
https://user:pass@host:port
host:port:user:pass
user:pass@host:port
host:port
```

## HTTP Server

Run as a microservice with REST API endpoints:

```bash
# Run with default port (3000)
cargo run --bin server

# Run with custom port
PORT=8080 cargo run --bin server

# Run with debug logging
RUST_LOG=debug cargo run --bin server
```

### API Endpoints

#### Health Check
```bash
curl http://localhost:3000/health
```

Response:
```json
{
  "status": "healthy",
  "version": "0.1.0"
}
```

#### Track Single Package
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
    },
    "all_events": [...]
  }
}
```

#### Track Multiple Packages (Batch)
```bash
curl -X POST http://localhost:3000/api/track/batch \
  -H "Content-Type: application/json" \
  -d '{
    "tracking_numbers": ["123456789012", "234567890123"],
    "carrier_code": 0
  }'
```

Response:
```json
{
  "success": true,
  "data": [
    { "tracking_number": "123...", "status": "...", ... },
    { "tracking_number": "234...", "status": "...", ... }
  ]
}
```

#### Get Server Metrics
```bash
curl http://localhost:3000/api/metrics
```

Response:
```json
{
  "total_requests": 1234,
  "requests_in_flight": 5,
  "uptime_seconds": 86400
}
```

### Environment Variables

- `PORT` - Server port (default: 3000)
- `RUST_LOG` - Log level: error, warn, info, debug, trace (default: info)

## Library Usage

### Basic Usage

```rust
use track17_rs::{Track17Client, carriers};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Client is Clone + Send + Sync - share across tasks!
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

    let tracking_numbers = vec!["123456789012", "234567890123", "345678901234"];

    // Spawn concurrent tasks
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

    // Collect results
    for handle in handles {
        match handle.await? {
            Ok(response) => {
                println!("Tracked: {}", response.shipments[0].number);
            }
            Err(e) => eprintln!("Error: {}", e),
        }
    }

    Ok(())
}
```

### With Proxy

```rust
use track17_rs::{Track17Client, ProxyConfig, carriers};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let proxy = ProxyConfig::parse("http://user:pass@proxy:8080");
    let client = Track17Client::with_proxy(proxy).await?;

    let response = client.track("1234567890", carriers::AUTO).await?;
    println!("Status: {:?}", response.shipments[0].shipment);

    Ok(())
}
```

## Architecture

### V8-Powered Credentials

The library uses an embedded V8 JavaScript runtime (via `deno_core`) to execute 17track's sign generation code. No browser automation required!

**Credential Flow:**
1. Fetch JS assets from 17track CDN (cached for 1 hour)
2. Execute sign module in V8 to generate authentication signature
3. Cache credentials across all client instances (1-hour TTL)
4. Automatically refresh when expired

### Thread Safety

The `Track17Client` is fully thread-safe:
- **Clone** - Cheap to clone, shares credential cache via `Arc<RwLock<>>`
- **Send + Sync** - Safe to share across threads and async tasks
- **Credential Sharing** - All clones share the same credentials
- **Concurrent Requests** - Multiple requests can run in parallel

### Performance

**First Request:** ~400-500ms (credential generation + tracking)
**Subsequent Requests:** ~100-200ms (credentials cached, tracking only)
**Batch Tracking:** ~10-20 concurrent requests internally

## Examples

See the `examples/` directory:
- `examples/concurrent_tracking.rs` - Concurrent tracking demonstration
- `examples/api_client.rs` - HTTP client example

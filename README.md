# track17-rs

Rust library and CLI for tracking packages via 17track.net private API.

## Features

- Auto-detect carrier or specify FedEx, UPS, USPS, DHL
- Batch tracking for multiple packages

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

## Library Usage

```rust
use track17_rs::{Track17Client, ProxyConfig, carriers};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let proxy = ProxyConfig::parse("http://user:pass@proxy:8080");
    let mut client = Track17Client::with_proxy(proxy).await?;

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

## Environment Variables

- `CHROME_PATH` - Custom Chrome/Chromium executable path

## Requirements

- Chrome or Chromium browser installed

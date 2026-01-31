use std::env;

use anyhow::Result;
use track17_rs::{ProxyConfig, Track17Client, carriers, format_location};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 2 {
        eprintln!("Usage: {} <tracking_numbers> [carrier] [proxy]", args[0]);
        eprintln!("  tracking_numbers: comma-separated (e.g., NUM1,NUM2,NUM3)");
        eprintln!("  carrier: auto, fedex, ups, usps, dhl (default: auto)");
        eprintln!("  proxy: http://user:pass@host:port or host:port:user:pass");
        std::process::exit(1);
    }

    // Parse comma-separated tracking numbers
    let tracking_numbers: Vec<String> = args[1]
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    if tracking_numbers.is_empty() {
        eprintln!("Error: No tracking numbers provided");
        std::process::exit(1);
    }

    let carrier = args.get(2).map(|s| s.as_str()).unwrap_or("auto");
    let carrier_code = match carrier.to_lowercase().as_str() {
        "auto" => carriers::AUTO,
        "fedex" => carriers::FEDEX,
        "ups" => carriers::UPS,
        "usps" => carriers::USPS,
        "dhl" => carriers::DHL,
        _ => {
            eprintln!("Unknown carrier: {}. Using auto-detect.", carrier);
            carriers::AUTO
        }
    };

    // Parse optional proxy
    let proxy = args.get(3).and_then(|s| {
        let config = ProxyConfig::parse(s);
        if config.is_none() {
            eprintln!(
                "Warning: Failed to parse proxy '{}', continuing without proxy",
                s
            );
        }
        config
    });

    let client = Track17Client::with_proxy(proxy).await?;

    println!("Tracking {} package(s)...", tracking_numbers.len());
    let response = client
        .track_multiple(&tracking_numbers, carrier_code)
        .await?;

    println!("Status: {} - {}", response.meta.code, response.meta.message);

    for shipment in &response.shipments {
        println!("\nTracking: {}", shipment.number);

        if let Some(details) = &shipment.shipment {
            // Try latest_event first, then fall back to tracking providers
            let latest = details.latest_event.as_ref().or_else(|| {
                details
                    .tracking
                    .as_ref()
                    .and_then(|t| t.providers.as_ref())
                    .and_then(|p| p.first())
                    .and_then(|p| p.events.first())
            });

            if let Some(event) = latest {
                let state = event.tracking_state();
                let time = event
                    .time_iso
                    .as_deref()
                    .or(event.time.as_deref())
                    .unwrap_or("N/A");
                println!("  Status: {}", state);
                println!(
                    "  Latest: {} - {}",
                    time,
                    event.description.as_deref().unwrap_or("N/A")
                );
                if let Some(raw_loc) = event.raw_location() {
                    let location = format_location(&raw_loc);
                    println!("  Location: {}", location);
                }
            }
        } else {
            // Show status based on response code
            match shipment.code {
                100 => println!("  Status: PENDING"),
                400 => println!("  Status: NOT_FOUND"),
                _ => println!("  Status: UNKNOWN (code {})", shipment.code),
            }
        }
    }

    Ok(())
}

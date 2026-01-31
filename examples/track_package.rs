use anyhow::Result;
use track17_rs::{Track17Client, carriers, format_location};

#[tokio::main]
async fn main() -> Result<()> {
    let client = Track17Client::new().await?;

    let response = client.track("123456789012", carriers::AUTO).await?;

    println!("Status: {} - {}", response.meta.code, response.meta.message);

    for shipment in &response.shipments {
        println!("\nTracking: {}", shipment.number);

        if let Some(details) = &shipment.shipment {
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
                    println!("  Location: {}", format_location(&raw_loc));
                }
            }
        } else {
            match shipment.code {
                100 => println!("  Status: PENDING"),
                400 => println!("  Status: NOT_FOUND"),
                _ => println!("  Status: UNKNOWN (code {})", shipment.code),
            }
        }
    }

    Ok(())
}

use std::time::Instant;

use anyhow::Result;
use track17_rs::{Track17Client, carriers};

#[tokio::main]
async fn main() -> Result<()> {
    // Create a single client instance
    let client = Track17Client::new().await?;

    // Test tracking numbers (you can replace with real tracking numbers)
    let tracking_numbers = vec![
        "123456789012",
        "234567890123",
        "345678901234",
        "456789012345",
        "567890123456",
        "678901234567",
        "789012345678",
        "890123456789",
        "901234567890",
        "012345678901",
    ];

    println!(
        "Tracking {} packages concurrently...",
        tracking_numbers.len()
    );
    let start = Instant::now();

    // Spawn concurrent tasks
    let handles: Vec<_> = tracking_numbers
        .iter()
        .map(|num| {
            let client = client.clone(); // Cheap clone (Arc)
            let num = num.to_string();
            tokio::spawn(async move { client.track(&num, carriers::AUTO).await })
        })
        .collect();

    // Wait for all tasks to complete
    let mut results = Vec::new();
    for handle in handles {
        match handle.await? {
            Ok(response) => results.push(response),
            Err(e) => eprintln!("Error tracking package: {}", e),
        }
    }

    let elapsed = start.elapsed();

    println!("\n=== Results ===");
    println!("Tracked {} packages in {:?}", results.len(), elapsed);
    println!(
        "Throughput: {:.2} packages/sec",
        results.len() as f64 / elapsed.as_secs_f64()
    );

    // Display results
    for (i, response) in results.iter().enumerate() {
        println!(
            "\n[{}] Status: {} - {}",
            i + 1,
            response.meta.code,
            response.meta.message
        );
        for shipment in &response.shipments {
            println!("  Tracking: {}", shipment.number);
            println!("  Code: {}", shipment.code);
        }
    }

    Ok(())
}

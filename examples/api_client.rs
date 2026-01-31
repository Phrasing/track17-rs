/// Example HTTP client demonstrating how to call the Track17 HTTP server API
///
/// Run the server first:
/// ```bash
/// cargo run --bin server
/// ```
///
/// Then run this example:
/// ```bash
/// cargo run --example api_client
/// ```

use serde::{Deserialize, Serialize};

#[derive(Serialize)]
struct TrackRequest {
    tracking_number: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    carrier_code: Option<u32>,
}

#[derive(Serialize)]
struct BatchTrackRequest {
    tracking_numbers: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    carrier_code: Option<u32>,
}

#[derive(Deserialize, Debug)]
struct TrackResponse {
    success: bool,
    data: TrackData,
}

#[derive(Deserialize, Debug)]
struct BatchTrackResponse {
    success: bool,
    data: Vec<TrackData>,
}

#[derive(Deserialize, Debug)]
struct TrackData {
    tracking_number: String,
    carrier: u32,
    status: String,
    latest_event: Option<EventData>,
    all_events: Vec<EventData>,
}

#[derive(Deserialize, Debug)]
struct EventData {
    time: String,
    description: String,
    location: Option<String>,
}

#[derive(Deserialize, Debug)]
struct HealthResponse {
    status: String,
    version: String,
}

#[derive(Deserialize, Debug)]
struct MetricsResponse {
    total_requests: u64,
    requests_in_flight: u64,
    uptime_seconds: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let base_url = std::env::var("API_URL").unwrap_or_else(|_| "http://localhost:3000".to_string());
    let client = reqwest::Client::new();

    println!("=== Track17 HTTP API Client Demo ===\n");

    // 1. Health Check
    println!("1. Checking server health...");
    let health_url = format!("{}/health", base_url);
    let health: HealthResponse = client.get(&health_url).send().await?.json().await?;
    println!("   Server status: {}", health.status);
    println!("   Version: {}\n", health.version);

    // 2. Track Single Package
    println!("2. Tracking single package...");
    let track_url = format!("{}/api/track", base_url);
    let request = TrackRequest {
        tracking_number: "123456789012".to_string(),
        carrier_code: None, // Auto-detect
    };

    match client.post(&track_url).json(&request).send().await {
        Ok(response) => {
            if response.status().is_success() {
                let result: TrackResponse = response.json().await?;
                println!("   Tracking Number: {}", result.data.tracking_number);
                println!("   Status: {}", result.data.status);
                println!("   Carrier: {}", result.data.carrier);

                if let Some(event) = &result.data.latest_event {
                    println!("\n   Latest Event:");
                    println!("     Time: {}", event.time);
                    println!("     Description: {}", event.description);
                    if let Some(location) = &event.location {
                        println!("     Location: {}", location);
                    }
                }

                println!("   Total events: {}\n", result.data.all_events.len());
            } else {
                let error_text = response.text().await?;
                println!("   Error: {}\n", error_text);
            }
        }
        Err(e) => {
            println!("   Request failed: {}\n", e);
        }
    }

    // 3. Track Multiple Packages (Batch)
    println!("3. Tracking multiple packages (batch)...");
    let batch_url = format!("{}/api/track/batch", base_url);
    let batch_request = BatchTrackRequest {
        tracking_numbers: vec![
            "123456789012".to_string(),
            "234567890123".to_string(),
            "345678901234".to_string(),
        ],
        carrier_code: None,
    };

    match client
        .post(&batch_url)
        .json(&batch_request)
        .send()
        .await
    {
        Ok(response) => {
            if response.status().is_success() {
                let result: BatchTrackResponse = response.json().await?;
                println!("   Successfully tracked {} packages:", result.data.len());
                for (i, track_data) in result.data.iter().enumerate() {
                    println!(
                        "   [{}] {} - Status: {}",
                        i + 1,
                        track_data.tracking_number,
                        track_data.status
                    );
                }
                println!();
            } else {
                let error_text = response.text().await?;
                println!("   Error: {}\n", error_text);
            }
        }
        Err(e) => {
            println!("   Request failed: {}\n", e);
        }
    }

    // 4. Get Metrics
    println!("4. Getting server metrics...");
    let metrics_url = format!("{}/api/metrics", base_url);
    let metrics: MetricsResponse = client.get(&metrics_url).send().await?.json().await?;
    println!("   Total requests: {}", metrics.total_requests);
    println!("   Requests in flight: {}", metrics.requests_in_flight);
    println!("   Uptime: {} seconds\n", metrics.uptime_seconds);

    println!("=== Demo Complete ===");

    Ok(())
}

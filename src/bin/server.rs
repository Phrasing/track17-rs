use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use anyhow::{Context, Result};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, trace::TraceLayer};
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

use track17_rs::types::TrackingEvent;
use track17_rs::{Shipment, Track17Client, carriers, format_location};

/// Server configuration
struct ServerConfig {
    port: u16,
}

impl ServerConfig {
    fn from_env() -> Self {
        Self {
            port: env::var("PORT")
                .ok()
                .and_then(|p| p.parse().ok())
                .unwrap_or(3000),
        }
    }
}

/// Application state shared across all requests
#[derive(Clone)]
struct AppState {
    client: Arc<Track17Client>,
    metrics: Arc<Metrics>,
}

/// Server metrics
struct Metrics {
    total_requests: AtomicU64,
    requests_in_flight: AtomicU64,
    start_time: Instant,
}

/// RAII guard for tracking in-flight requests
struct RequestGuard<'a>(&'a AtomicU64);

impl<'a> Drop for RequestGuard<'a> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::Relaxed);
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize tracing/logging
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "server=info,tower_http=debug".into()),
        )
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Read configuration from environment
    let config = ServerConfig::from_env();

    // Initialize shared Track17Client
    tracing::info!("Initializing Track17 client...");
    let track_client = Arc::new(
        Track17Client::new()
            .await
            .context("Failed to initialize Track17 client")?,
    );
    tracing::info!("Track17 client initialized successfully");

    // Build Axum app with routes
    let app = build_app(track_client);

    // Bind server
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("Server listening on {}", addr);

    // Run server with graceful shutdown
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("Server error")?;

    tracing::info!("Server shut down gracefully");
    Ok(())
}

/// Build the Axum application with routes and middleware
fn build_app(client: Arc<Track17Client>) -> Router {
    let metrics = Arc::new(Metrics {
        total_requests: AtomicU64::new(0),
        requests_in_flight: AtomicU64::new(0),
        start_time: Instant::now(),
    });

    let state = AppState { client, metrics };

    Router::new()
        // Health check
        .route("/health", get(health_check))
        // API routes
        .route("/api/track", post(track_single))
        .route("/api/track/batch", post(track_batch))
        .route("/api/metrics", get(get_metrics))
        // Middleware
        .layer(
            ServiceBuilder::new()
                .layer(TraceLayer::new_for_http())
                .layer(CorsLayer::permissive()),
        )
        .with_state(state)
}

/// Health check endpoint
async fn health_check() -> Json<HealthResponse> {
    Json(HealthResponse {
        status: "healthy".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    })
}

#[derive(Serialize)]
struct HealthResponse {
    status: String,
    version: String,
}

/// Track a single package
async fn track_single(
    State(state): State<AppState>,
    Json(request): Json<TrackRequest>,
) -> Result<Json<TrackResponse>, ApiError> {
    // Increment metrics
    state.metrics.total_requests.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .requests_in_flight
        .fetch_add(1, Ordering::Relaxed);

    // Ensure we decrement on exit
    let _guard = RequestGuard(&state.metrics.requests_in_flight);

    let carrier_code = request.carrier_code.unwrap_or(carriers::AUTO);

    tracing::info!(
        "Tracking package: {} with carrier {}",
        request.tracking_number,
        carrier_code
    );

    // Call tracking client
    let response = state
        .client
        .track(&request.tracking_number, carrier_code)
        .await
        .map_err(|e| {
            tracing::error!("Tracking error: {}", e);
            ApiError::InternalError(e.to_string())
        })?;

    // Transform response
    let shipment = response
        .shipments
        .first()
        .ok_or_else(|| ApiError::NotFound("No tracking data found for this package".to_string()))?;

    Ok(Json(TrackResponse {
        success: true,
        data: TrackData::from_shipment(shipment),
    }))
}

#[derive(Deserialize)]
struct TrackRequest {
    tracking_number: String,
    #[serde(default)]
    carrier_code: Option<u32>,
}

#[derive(Serialize)]
struct TrackResponse {
    success: bool,
    data: TrackData,
}

/// Track multiple packages (batch)
async fn track_batch(
    State(state): State<AppState>,
    Json(request): Json<BatchTrackRequest>,
) -> Result<Json<BatchTrackResponse>, ApiError> {
    state.metrics.total_requests.fetch_add(1, Ordering::Relaxed);
    state
        .metrics
        .requests_in_flight
        .fetch_add(1, Ordering::Relaxed);
    let _guard = RequestGuard(&state.metrics.requests_in_flight);

    if request.tracking_numbers.is_empty() {
        return Err(ApiError::BadRequest(
            "tracking_numbers cannot be empty".to_string(),
        ));
    }

    let carrier_code = request.carrier_code.unwrap_or(carriers::AUTO);

    tracing::info!(
        "Batch tracking {} packages with carrier {}",
        request.tracking_numbers.len(),
        carrier_code
    );

    // Use existing track_multiple method (already concurrent!)
    let response = state
        .client
        .track_multiple(&request.tracking_numbers, carrier_code)
        .await
        .map_err(|e| {
            tracing::error!("Batch tracking error: {}", e);
            ApiError::InternalError(e.to_string())
        })?;

    let data = response
        .shipments
        .iter()
        .map(TrackData::from_shipment)
        .collect();

    Ok(Json(BatchTrackResponse {
        success: true,
        data,
    }))
}

#[derive(Deserialize)]
struct BatchTrackRequest {
    tracking_numbers: Vec<String>,
    #[serde(default)]
    carrier_code: Option<u32>,
}

#[derive(Serialize)]
struct BatchTrackResponse {
    success: bool,
    data: Vec<TrackData>,
}

/// Get server metrics
async fn get_metrics(State(state): State<AppState>) -> Json<MetricsResponse> {
    Json(MetricsResponse {
        total_requests: state.metrics.total_requests.load(Ordering::Relaxed),
        requests_in_flight: state.metrics.requests_in_flight.load(Ordering::Relaxed),
        uptime_seconds: state.metrics.start_time.elapsed().as_secs(),
    })
}

#[derive(Serialize)]
struct MetricsResponse {
    total_requests: u64,
    requests_in_flight: u64,
    uptime_seconds: u64,
}

/// Tracking data for API response
#[derive(Serialize)]
struct TrackData {
    tracking_number: String,
    carrier: u32,
    status: String,
    latest_event: Option<EventData>,
    all_events: Vec<EventData>,
}

#[derive(Serialize)]
struct EventData {
    time: String,
    description: String,
    location: Option<String>,
}

impl TrackData {
    fn from_shipment(shipment: &Shipment) -> Self {
        let latest_event = shipment
            .shipment
            .as_ref()
            .and_then(|s| s.latest_event.as_ref())
            .map(EventData::from_tracking_event);

        let all_events = shipment
            .shipment
            .as_ref()
            .and_then(|s| s.tracking.as_ref())
            .and_then(|t| t.providers.as_ref())
            .and_then(|p| p.first())
            .map(|provider| {
                provider
                    .events
                    .iter()
                    .map(EventData::from_tracking_event)
                    .collect()
            })
            .unwrap_or_default();

        Self {
            tracking_number: shipment.number.clone(),
            carrier: shipment.carrier,
            status: shipment
                .shipment
                .as_ref()
                .and_then(|s| s.latest_event.as_ref())
                .map(|e| e.tracking_state().to_string())
                .unwrap_or_else(|| "UNKNOWN".to_string()),
            latest_event,
            all_events,
        }
    }
}

impl EventData {
    fn from_tracking_event(event: &TrackingEvent) -> Self {
        Self {
            time: event
                .time_iso
                .clone()
                .or_else(|| event.time.clone())
                .unwrap_or_else(|| "N/A".to_string()),
            description: event
                .description
                .clone()
                .unwrap_or_else(|| "N/A".to_string()),
            location: event
                .raw_location()
                .map(|loc| format_location(loc.as_str())),
        }
    }
}

/// API error types
enum ApiError {
    BadRequest(String),
    NotFound(String),
    InternalError(String),
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, message) = match self {
            ApiError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            ApiError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ApiError::InternalError(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        let body = Json(serde_json::json!({
            "success": false,
            "error": message
        }));

        (status, body).into_response()
    }
}

/// Graceful shutdown signal handler
async fn shutdown_signal() {
    use tokio::signal;

    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install signal handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {
            tracing::info!("Received Ctrl+C, shutting down gracefully...");
        }
        _ = terminate => {
            tracing::info!("Received SIGTERM, shutting down gracefully...");
        }
    }
}

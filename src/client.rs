use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use base64::Engine;
use chaser_oxide::{
    Browser, BrowserConfig,
    cdp::browser_protocol::fetch::{
        ContinueRequestParams, EnableParams, EventRequestPaused, RequestPattern,
    },
    chaser::ChaserPage,
    profiles::ChaserProfile,
};
use futures::StreamExt;
use tokio::sync::Mutex;
use tokio::time::timeout;
use wreq::{Client, header};
use wreq_util::Emulation;

use crate::local_proxy::LocalProxy;
use crate::proxy::ProxyConfig;
use crate::types::{Shipment, TrackingItem, TrackingRequest, TrackingResponse, carriers};

const API_URL: &str = "https://t.17track.net/track/restapi";

/// Extract sign from a paused request's POST data
fn extract_sign_from_event(event: &EventRequestPaused) -> Option<String> {
    event
        .request
        .post_data_entries
        .as_ref()?
        .iter()
        .find_map(|entry| {
            let b64_body: &str = entry.bytes.as_ref()?.as_ref();
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(b64_body)
                .ok()?;
            let body = String::from_utf8(decoded).ok()?;
            let json: serde_json::Value = serde_json::from_str(&body).ok()?;
            json.get("sign")?.as_str().map(|s| s.to_string())
        })
}
const INVALID_SIGN_CODE: i32 = -11;
const INVALID_SESSION_CODE: i32 = -14; // Session/cookie expired (empty shipments, empty guid)
const PENDING_SHIPMENT_CODE: i32 = 100;
const NOT_FOUND_SHIPMENT_CODE: i32 = 400;
const EXTRACTION_TIMEOUT: Duration = Duration::from_secs(15);
const PENDING_RETRY_DELAY: Duration = Duration::from_secs(2);
const MAX_PENDING_RETRIES: u32 = 50; // New tracking numbers can take ~100 seconds to fetch

#[derive(Debug, Clone)]
pub struct ApiCredentials {
    pub sign: String,
    pub last_event_id: String,
    pub yq_bid: String,
}

/// Configuration for Track17Client
#[derive(Debug, Clone, Default)]
pub struct Track17Config {
    /// Proxy configuration
    pub proxy: Option<ProxyConfig>,
    /// Custom Chrome executable path (overrides CHROME_PATH env var)
    pub chrome_path: Option<PathBuf>,
    /// Skip process-reducing Chrome flags (not recommended)
    pub skip_process_optimization: bool,
}

/// Track17 client that uses Chrome only for credential extraction.
///
/// Chrome is launched briefly to extract API credentials (sign, cookies),
/// then immediately closed. Subsequent tracking requests use HTTP only.
/// Chrome is only relaunched when credentials expire (API returns code -11).
pub struct Track17Client {
    http_client: Client,
    config: Track17Config,
    credentials: Option<ApiCredentials>,
    /// Mutex to prevent concurrent Chrome launches during credential extraction
    credential_mutex: Arc<Mutex<()>>,
}

impl Track17Client {
    pub async fn new() -> Result<Self> {
        Self::with_config(Track17Config::default()).await
    }

    pub async fn with_proxy(proxy: Option<ProxyConfig>) -> Result<Self> {
        Self::with_config(Track17Config {
            proxy,
            ..Default::default()
        })
        .await
    }

    pub async fn with_config(config: Track17Config) -> Result<Self> {
        // Build HTTP client with optional proxy
        let mut http_builder = Client::builder()
            .emulation(Emulation::Chrome143)
            .cookie_store(true)
            .gzip(true)
            .brotli(true)
            .zstd(true);

        if let Some(ref proxy) = config.proxy {
            let proxy_url = proxy.to_url();
            http_builder = http_builder.proxy(wreq::Proxy::all(&proxy_url)?);
        }

        let http_client = http_builder.build()?;

        // Verify proxy by checking external IP
        if config.proxy.is_some()
            && let Ok(resp) = http_client.get("https://httpbin.org/ip").send().await
            && let Ok(body) = resp.text().await
            && let Ok(json) = serde_json::from_str::<serde_json::Value>(&body)
            && let Some(ip) = json.get("origin").and_then(|v| v.as_str())
        {
            eprintln!("Proxy IP: {}", ip);
        }

        Ok(Self {
            http_client,
            config,
            credentials: None,
            credential_mutex: Arc::new(Mutex::new(())),
        })
    }

    /// Close the client and clean up resources.
    /// Since Chrome is closed immediately after credential extraction,
    /// this method mainly exists for API compatibility.
    pub async fn close(self) -> Result<()> {
        // Nothing to close - Chrome is not kept alive
        Ok(())
    }

    /// Build browser configuration with optional proxy
    fn build_browser_config_with_proxy(&self, proxy_addr: Option<&str>) -> Result<BrowserConfig> {
        let mut builder = BrowserConfig::builder().new_headless_mode().incognito();

        // Add proxy if provided
        if let Some(addr) = proxy_addr {
            builder = builder.arg(format!("--proxy-server={}", addr));
        }

        // Add process-reducing flags unless explicitly skipped
        if !self.config.skip_process_optimization {
            builder = builder
                .arg("--disable-gpu")
                .arg("--disable-dev-shm-usage")
                .arg("--disable-software-rasterizer")
                .arg("--disable-extensions")
                .arg("--disable-background-networking")
                .arg("--disable-sync")
                .arg("--disable-translate")
                .arg("--metrics-recording-only")
                .arg("--no-first-run")
                .arg("--mute-audio");
        }

        // Chrome path: config takes precedence over env var
        if let Some(ref chrome_path) = self.config.chrome_path {
            builder = builder.chrome_executable(chrome_path);
        } else if let Ok(chrome_path) = std::env::var("CHROME_PATH") {
            builder = builder.chrome_executable(chrome_path);
        }

        builder
            .build()
            .map_err(|e| anyhow::anyhow!("Failed to build browser config: {}", e))
    }

    /// Extract credentials by launching Chrome, navigating to 17track, and closing Chrome.
    /// This method serializes concurrent calls to prevent multiple Chrome launches.
    async fn extract_credentials(&mut self, tracking_number: &str) -> Result<ApiCredentials> {
        // Acquire mutex to prevent concurrent Chrome launches
        let _lock = self.credential_mutex.lock().await;

        // Double-check if another call already extracted credentials
        if let Some(ref creds) = self.credentials {
            return Ok(creds.clone());
        }

        eprintln!("Launching Chrome to extract credentials...");

        // Handle proxy configuration
        let mut local_proxy_task = None;
        let browser_config = if let Some(ref proxy) = self.config.proxy {
            if proxy.username.is_some() {
                // Start local proxy for authenticated upstream
                let local_proxy = LocalProxy::start(proxy.clone()).await?;
                let local_addr = local_proxy.local_addr();
                eprintln!(
                    "Using proxy: {} (via local {})",
                    proxy.to_host_port(),
                    local_addr
                );
                local_proxy_task = Some(local_proxy.run());
                self.build_browser_config_with_proxy(Some(&local_addr))?
            } else {
                // Direct proxy (no auth needed)
                eprintln!("Using proxy: {}", proxy.to_host_port());
                self.build_browser_config_with_proxy(Some(&proxy.to_host_port()))?
            }
        } else {
            // No proxy
            self.build_browser_config_with_proxy(None)?
        };

        // Launch browser
        let (mut browser, mut handler) = Browser::launch(browser_config)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to launch browser: {}", e))?;

        let handler_task = tokio::spawn(async move { while handler.next().await.is_some() {} });

        // Extract credentials
        let result = self.do_extract_credentials(&browser, tracking_number).await;

        // ALWAYS close browser, even on error
        eprintln!("Closing Chrome...");
        if let Err(e) = browser.close().await {
            eprintln!("Warning: Failed to close browser: {}", e);
        }
        handler_task.abort();
        if let Some(proxy_task) = local_proxy_task {
            proxy_task.abort();
        }
        eprintln!("Chrome closed");

        let credentials = result?;
        self.credentials = Some(credentials.clone());
        Ok(credentials)
    }

    /// Internal credential extraction logic (browser already launched)
    async fn do_extract_credentials(
        &self,
        browser: &Browser,
        tracking_number: &str,
    ) -> Result<ApiCredentials> {
        let page = browser.new_page("about:blank").await?;
        let chaser = ChaserPage::new(page.clone());

        let profile = ChaserProfile::windows().build();
        chaser.apply_profile(&profile).await?;

        // Enable Fetch interception for the API endpoint only
        let pattern = RequestPattern::builder()
            .url_pattern("*t.17track.net/track/restapi*")
            .build();
        page.execute(EnableParams::builder().pattern(pattern).build())
            .await?;

        // Subscribe to request paused events
        let mut events = page.event_listener::<EventRequestPaused>().await?;

        eprintln!("Extracting credentials...");
        let url = format!("https://t.17track.net/en#nums={}", tracking_number);

        // Start navigation in background
        let page_clone = page.clone();
        let nav_handle = tokio::spawn(async move {
            let chaser = ChaserPage::new(page_clone);
            let _ = chaser.goto(&url).await;
        });

        // Wait for the API request to be intercepted (event-driven, no polling)
        let sign = timeout(EXTRACTION_TIMEOUT, async {
            while let Some(event) = events.next().await {
                let _ = page
                    .execute(ContinueRequestParams::new(event.request_id.clone()))
                    .await;

                if let Some(sign) = extract_sign_from_event(&event) {
                    return Ok::<_, anyhow::Error>(sign);
                }
            }
            anyhow::bail!("Event stream ended without capturing sign")
        })
        .await
        .map_err(|_| anyhow::anyhow!("Timeout waiting for API request"))??;

        // Wait for navigation to complete
        let _ = nav_handle.await;

        // Extract cookies from page
        let extract_cookies_js = r#"
            (function() {
                const cookies = document.cookie;
                let lastEventId = '', yqBid = '';
                cookies.split(';').forEach(c => {
                    const parts = c.trim().split('=');
                    const name = parts[0];
                    const value = parts.slice(1).join('=');
                    if (name === 'Last-Event-ID') lastEventId = decodeURIComponent(value);
                    if (name === '_yq_bid') yqBid = value;
                });
                return JSON.stringify({ lastEventId, yqBid });
            })()
        "#;

        let cookies: serde_json::Value = chaser
            .evaluate(extract_cookies_js)
            .await?
            .ok_or_else(|| anyhow::anyhow!("Failed to extract cookies"))?;

        let last_event_id = cookies["lastEventId"].as_str().unwrap_or("").to_string();
        let yq_bid = cookies["yqBid"].as_str().unwrap_or("").to_string();

        eprintln!("Credentials captured!");
        Ok(ApiCredentials {
            sign,
            last_event_id,
            yq_bid,
        })
    }

    /// Clear cached credentials, forcing re-extraction on next request
    pub fn clear_credentials(&mut self) {
        self.credentials = None;
    }

    pub async fn track(
        &mut self,
        tracking_number: &str,
        carrier_code: u32,
    ) -> Result<TrackingResponse> {
        self.track_multiple(&[tracking_number.to_string()], carrier_code)
            .await
    }

    /// Make a single API request for tracking numbers
    async fn make_request(&self, items: &[TrackingItem], guid: &str) -> Result<TrackingResponse> {
        let creds = self.credentials.as_ref().unwrap().clone();

        // Log request details
        eprintln!(
            "[track17-req] items={:?}, guid={}, sign_len={}, last_event_id_len={}, yq_bid_len={}",
            items
                .iter()
                .map(|i| format!("{}:{}", i.num, i.fc))
                .collect::<Vec<_>>(),
            if guid.is_empty() {
                "(empty)"
            } else {
                &guid[..guid.len().min(8)]
            },
            creds.sign.len(),
            creds.last_event_id.len(),
            creds.yq_bid.len(),
        );

        let request = TrackingRequest {
            data: items.to_vec(),
            guid: guid.to_string(),
            time_zone_offset: -480,
            sign: creds.sign.clone(),
        };

        let cookies = format!(
            "country=US; _yq_bid={}; v5_Culture=en; Last-Event-ID={}",
            creds.yq_bid, creds.last_event_id
        );

        let response = self
            .http_client
            .post(API_URL)
            .header(header::REFERER, "https://t.17track.net/en")
            .header("last-event-id", &creds.last_event_id)
            .header(header::COOKIE, &cookies)
            .header(header::ORIGIN, "https://t.17track.net")
            .body(serde_json::to_string(&request)?)
            .send()
            .await?;

        let status = response.status();
        let body = response.text().await?;

        // Log raw response (truncated for readability)
        eprintln!(
            "[track17-resp] status={}, body_len={}, body_preview={}",
            status,
            body.len(),
            &body[..body.len().min(500)]
        );

        if !status.is_success() {
            anyhow::bail!("API request failed: {} {}", status, body);
        }

        serde_json::from_str(&body).map_err(|e| anyhow::anyhow!("Failed to parse response: {}", e))
    }

    /// Check if a shipment needs more polling
    fn shipment_needs_retry(shipment: &Shipment) -> bool {
        // Code 100 = pending registration
        if shipment.code == PENDING_SHIPMENT_CODE {
            return true;
        }
        // Code 200 but missing or empty shipment data
        if shipment.code == 200 {
            match &shipment.shipment {
                None => return true,
                Some(details) => {
                    let has_events = details.latest_event.is_some()
                        || details
                            .tracking
                            .as_ref()
                            .and_then(|t| t.providers.as_ref())
                            .map(|p| p.iter().any(|prov| !prov.events.is_empty()))
                            .unwrap_or(false);
                    return !has_events;
                }
            }
        }
        false
    }

    /// Extract suggested carrier from code 400 response
    fn get_suggested_carrier(shipment: &Shipment) -> Option<u32> {
        shipment.extra.as_ref()?.iter().find_map(|e| {
            // Prefer FedEx if available, otherwise take first carrier
            if e.multi.contains(&carriers::FEDEX) {
                Some(carriers::FEDEX)
            } else if e.multi.contains(&carriers::UPS) {
                Some(carriers::UPS)
            } else if e.multi.contains(&carriers::USPS) {
                Some(carriers::USPS)
            } else {
                e.multi.first().copied()
            }
        })
    }

    pub async fn track_multiple(
        &mut self,
        tracking_numbers: &[String],
        carrier_code: u32,
    ) -> Result<TrackingResponse> {
        // Get credentials, extracting if needed (launches Chrome briefly)
        if self.credentials.is_none() {
            self.extract_credentials(&tracking_numbers[0]).await?;
        }

        let mut pending_retries = 0;
        let mut session_guid = String::new();

        // Track state per tracking number: (number, carrier, resolved_shipment)
        let mut items: Vec<TrackingItem> = tracking_numbers
            .iter()
            .map(|num| TrackingItem {
                num: num.clone(),
                fc: carrier_code,
                sc: 0,
            })
            .collect();

        // Final results map: number -> shipment
        let mut final_shipments: std::collections::HashMap<String, Shipment> =
            std::collections::HashMap::new();

        loop {
            // Filter to items not yet resolved
            let pending_items: Vec<TrackingItem> = items
                .iter()
                .filter(|item| !final_shipments.contains_key(&item.num))
                .cloned()
                .collect();

            if pending_items.is_empty() {
                break;
            }

            let response = self.make_request(&pending_items, &session_guid).await?;

            // Log parsed response details
            eprintln!(
                "[track17-parsed] meta.code={}, meta.message={}, guid={}, shipments: [{}]",
                response.meta.code,
                response.meta.message,
                if response.guid.is_empty() {
                    "(empty)"
                } else {
                    &response.guid[..response.guid.len().min(8)]
                },
                response
                    .shipments
                    .iter()
                    .map(|s| format!(
                        "{}:code={},has_shipment={},has_events={}",
                        s.number,
                        s.code,
                        s.shipment.is_some(),
                        s.shipment
                            .as_ref()
                            .map(|d| d.latest_event.is_some()
                                || d.tracking
                                    .as_ref()
                                    .and_then(|t| t.providers.as_ref())
                                    .map(|p| p.iter().any(|prov| !prov.events.is_empty()))
                                    .unwrap_or(false))
                            .unwrap_or(false)
                    ))
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            // Handle sign/session expiration - need to re-extract credentials (launches Chrome briefly)
            // Code -11: Invalid sign (signature expired)
            // Code -14: Invalid session (cookies expired, returns empty shipments/guid)
            if response.meta.code == INVALID_SIGN_CODE || response.meta.code == INVALID_SESSION_CODE
            {
                eprintln!(
                    "Credentials expired (code {}), refreshing...",
                    response.meta.code
                );
                self.credentials = None;
                self.extract_credentials(&tracking_numbers[0]).await?;
                continue;
            }

            // Store GUID for subsequent requests
            if !response.guid.is_empty() {
                session_guid = response.guid.clone();
            }

            // Process each shipment
            for shipment in response.shipments {
                let num = shipment.number.clone();

                // Code 400 with carrier suggestions - retry with suggested carrier
                if shipment.code == NOT_FOUND_SHIPMENT_CODE
                    && let Some(suggested) = Self::get_suggested_carrier(&shipment)
                {
                    eprintln!(
                        "Auto-detect failed for {}, retrying with carrier {}",
                        num, suggested
                    );
                    // Update the item's carrier for next iteration
                    if let Some(item) = items.iter_mut().find(|i| i.num == num) {
                        item.fc = suggested;
                    }
                    continue;
                }

                // Check if this shipment is complete
                if !Self::shipment_needs_retry(&shipment) {
                    final_shipments.insert(num, shipment);
                }
            }

            // Check if we still have pending items that need retry
            let still_pending = items
                .iter()
                .filter(|item| !final_shipments.contains_key(&item.num))
                .count();

            if still_pending > 0 {
                // Log retry decision
                eprintln!(
                    "[track17-retry] pending={}, retry_count={}/{}",
                    still_pending,
                    pending_retries + 1,
                    MAX_PENDING_RETRIES
                );

                if pending_retries >= MAX_PENDING_RETRIES {
                    // Max retries reached, add remaining as-is
                    eprintln!("Max retries reached, returning partial results");
                    for item in &items {
                        if !final_shipments.contains_key(&item.num) {
                            // Create a placeholder shipment
                            final_shipments.insert(
                                item.num.clone(),
                                Shipment {
                                    code: PENDING_SHIPMENT_CODE,
                                    number: item.num.clone(),
                                    carrier: item.fc,
                                    carrier_final: None,
                                    param: None,
                                    params: None,
                                    params_v2: None,
                                    extra: None,
                                    shipment: None,
                                    pre_status: None,
                                    prior_status: None,
                                    state: None,
                                    state_final: None,
                                    service_type: None,
                                    service_type_final: None,
                                    key: None,
                                    show_more: false,
                                },
                            );
                        }
                    }
                    break;
                }

                pending_retries += 1;
                eprintln!(
                    "Tracking data incomplete for {} package(s), retrying ({}/{})...",
                    still_pending, pending_retries, MAX_PENDING_RETRIES
                );
                tokio::time::sleep(PENDING_RETRY_DELAY).await;
            }
        }

        // Build final response preserving original order
        let shipments: Vec<Shipment> = tracking_numbers
            .iter()
            .filter_map(|num| final_shipments.remove(num))
            .collect();

        Ok(TrackingResponse {
            id: 0,
            guid: session_guid,
            shipments,
            meta: crate::types::Meta {
                code: 200,
                message: "Ok".to_string(),
            },
        })
    }
}

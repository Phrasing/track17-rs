//! Thread-safe credential cache for concurrent access.
//!
//! This module provides a thread-safe cache for API credentials and JS assets.
//! Multiple client instances can share the same cache via Arc<RwLock<>>,
//! enabling efficient credential sharing across threads while minimizing regeneration overhead.
//!
//! Note: V8 runtime is not cached because it's not Send/Sync (contains Rc/RefCell).
//! A fresh runtime is created for each credential generation.

use std::sync::Arc;
use tokio::sync::RwLock;

use anyhow::{Context, Result};
use wreq::Client;

use crate::credential::ApiCredentials;
use crate::js_fetcher::{self, JsAssets};
use crate::js_runtime::SignGenerator;
use crate::last_event_id::{self, LastEventIdConfig};
use crate::yq_bid;

/// Thread-safe credential cache shared across all client clones.
///
/// This cache stores:
/// - API credentials (sign, yq_bid, configs_md5)
/// - JS assets fetched from CDN (1-hour TTL)
///
/// The cache uses `Arc<RwLock<>>` to allow multiple concurrent readers (tracking requests)
/// while ensuring only one writer can regenerate credentials at a time.
///
/// Note: V8 runtime is not cached because it's not thread-safe (not Send/Sync).
/// A fresh runtime is created for each credential generation (~400ms overhead).
///
/// # Example
///
/// ```no_run
/// use track17_rs::CredentialCache;
/// use wreq::Client;
///
/// #[tokio::main]
/// async fn main() {
///     let cache = CredentialCache::new();
///     let client = Client::builder().build().unwrap();
///
///     // Fast path: read lock (if credentials are valid)
///     if let Some(creds) = cache.get_valid_credentials().await {
///         println!("Using cached credentials");
///     }
///
///     // Slow path: write lock (if credentials expired)
///     let creds = cache.refresh_credentials(&client).await.unwrap();
///     println!("Generated fresh credentials");
/// }
/// ```
#[derive(Clone)]
pub struct CredentialCache {
    inner: Arc<RwLock<CredentialCacheInner>>,
}

struct CredentialCacheInner {
    credentials: Option<ApiCredentials>,
    cached_assets: Option<JsAssets>,
    yq_bid: String,
}

impl CredentialCache {
    /// Create a new credential cache.
    ///
    /// Generates a fresh `_yq_bid` device identifier that will be reused
    /// for all credentials generated from this cache.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(CredentialCacheInner {
                credentials: None,
                cached_assets: None,
                yq_bid: yq_bid::generate_yq_bid(),
            })),
        }
    }

    /// Get valid credentials if available (fast path with read lock).
    ///
    /// Returns `Some(credentials)` if credentials are cached and JS assets are still fresh.
    /// Returns `None` if credentials are missing or expired.
    ///
    /// This method uses a read lock, allowing multiple threads to check credentials
    /// concurrently without blocking each other.
    pub async fn get_valid_credentials(&self) -> Option<ApiCredentials> {
        let cache = self.inner.read().await;

        // Check if credentials are still valid
        if let Some(ref creds) = cache.credentials
            && cache
                .cached_assets
                .as_ref()
                .map(|a| a.is_fresh())
                .unwrap_or(false)
        {
            return Some(creds.clone());
        }

        None
    }

    /// Refresh credentials (slow path with write lock).
    ///
    /// This method:
    /// 1. Acquires a write lock (blocks other readers and writers)
    /// 2. Double-checks if another thread already regenerated credentials
    /// 3. Fetches or reuses cached JS assets (1-hour TTL)
    /// 4. Creates a fresh V8 runtime (~400ms initialization)
    /// 5. Generates fresh credentials
    ///
    /// The double-check pattern prevents thundering herd: if multiple threads
    /// detect expired credentials simultaneously, only the first one regenerates.
    pub async fn refresh_credentials(&self, http_client: &Client) -> Result<ApiCredentials> {
        // Step 1: Check if we need to refresh and get/fetch assets
        let (assets, yq_bid) = {
            let cache = self.inner.write().await;

            // Double-check: another thread may have regenerated while we waited
            if let Some(ref creds) = cache.credentials
                && cache
                    .cached_assets
                    .as_ref()
                    .map(|a| a.is_fresh())
                    .unwrap_or(false)
            {
                eprintln!("[credential_cache] Another thread already refreshed credentials");
                return Ok(creds.clone());
            }

            eprintln!("[credential_cache] Refreshing credentials...");

            // Fetch or reuse JS assets (1-hour cache)
            if let Some(ref cached) = cache.cached_assets {
                if cached.is_fresh() {
                    eprintln!(
                        "[credential_cache] Reusing cached JS assets (age: {:?})",
                        cached.fetched_at.elapsed()
                    );
                    let assets = cached.clone();
                    let yq_bid = cache.yq_bid.clone();
                    (assets, yq_bid)
                } else {
                    eprintln!("[credential_cache] JS assets expired, re-fetching...");
                    drop(cache); // Release lock before async operation
                    let new_assets = js_fetcher::fetch_js_assets(http_client)
                        .await
                        .context("Failed to fetch JS assets from CDN")?;
                    let mut cache = self.inner.write().await;
                    cache.cached_assets = Some(new_assets.clone());
                    let yq_bid = cache.yq_bid.clone();
                    (new_assets, yq_bid)
                }
            } else {
                eprintln!("[credential_cache] Fetching JS assets for first time...");
                drop(cache); // Release lock before async operation
                let new_assets = js_fetcher::fetch_js_assets(http_client)
                    .await
                    .context("Failed to fetch JS assets from CDN")?;
                let mut cache = self.inner.write().await;
                cache.cached_assets = Some(new_assets.clone());
                let yq_bid = cache.yq_bid.clone();
                (new_assets, yq_bid)
            }
        }; // Lock released here

        // Step 2: Generate credentials using V8 in a blocking task
        // V8 is not Send/Sync, so we run it in a dedicated blocking thread
        let sign_module_js = assets.sign_module_js.clone();
        let sign = tokio::task::spawn_blocking(move || {
            use futures::executor::block_on;

            eprintln!("[credential_cache] Creating fresh V8 runtime...");
            let mut generator = SignGenerator::new().context("Failed to create V8 runtime")?;

            eprintln!("[credential_cache] Initializing V8 runtime...");
            block_on(generator.initialize(&sign_module_js))
                .context("Failed to initialize sign module in V8")?;

            eprintln!("[credential_cache] Generating sign...");
            let sign =
                block_on(generator.generate_sign()).context("Failed to generate sign from V8")?;

            if sign.is_empty() {
                anyhow::bail!("V8 returned empty sign");
            }

            eprintln!("[credential_cache] Sign generated: {} chars", sign.len());

            Ok::<String, anyhow::Error>(sign)
        })
        .await
        .context("V8 task panicked")??;

        // Step 3: Store credentials in cache
        let credentials = ApiCredentials {
            sign,
            last_event_id: String::new(), // Computed per-request in make_request
            yq_bid,
            configs_md5: assets.configs_md5.clone(),
        };

        {
            let mut cache = self.inner.write().await;
            cache.credentials = Some(credentials.clone());
        } // Lock released

        eprintln!("[credential_cache] Credentials refreshed successfully");
        Ok(credentials)
    }

    /// Invalidate the cache (credentials, assets, and runtime).
    ///
    /// This is called when the API returns error codes indicating credentials are expired:
    /// - Code -11 (invalid sign)
    /// - Code -14 (invalid session)
    /// - Code -5 (invalid uIP)
    ///
    /// Dropping the cached runtime ensures fresh state for the next credential generation.
    pub async fn invalidate(&self) {
        let mut cache = self.inner.write().await;
        eprintln!("[credential_cache] Invalidating cache (assets + credentials)");
        cache.credentials = None;
        cache.cached_assets = None;
    }

    /// Generate the Last-Event-ID for a specific request body.
    ///
    /// This must be called per-request because the header includes a hash of the body.
    /// Only needed when `guid` is empty (first request).
    pub async fn generate_last_event_id_for_body(&self, request_body_json: &str) -> Result<String> {
        let cache = self.inner.read().await;

        let configs_md5 = cache
            .cached_assets
            .as_ref()
            .map(|a| a.configs_md5.clone())
            .unwrap_or_else(|| "1.0.156".to_string());

        let config = LastEventIdConfig {
            yq_bid: cache.yq_bid.clone(),
            configs_md5,
            ..Default::default()
        };

        Ok(last_event_id::generate_last_event_id(
            request_body_json,
            &config,
        ))
    }
}

impl Default for CredentialCache {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_cache_creation() {
        let cache = CredentialCache::new();
        assert!(cache.get_valid_credentials().await.is_none());
    }

    #[tokio::test]
    async fn test_invalidation() {
        let cache = CredentialCache::new();

        // Invalidate should succeed even if cache is empty
        cache.invalidate().await;

        assert!(cache.get_valid_credentials().await.is_none());
    }
}

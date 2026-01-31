//! Fetches JavaScript assets from 17track's CDN for sign generation.
//!
//! The tracking page at `t.17track.net/en` serves a Next.js App Router page with
//! JS chunks hosted on `static.17track.net`. The sign generator (chunk 839) is
//! dynamically loaded - its URL is only discoverable from the webpack runtime.
//!
//! Flow:
//! 1. Fetch the tracking page HTML
//! 2. Extract configs.md5 and CDN base URL from HTML
//! 3. Find and fetch the webpack runtime JS (has `id="_R_"`)
//! 4. Extract chunk 839's filename from the webpack runtime's `r.u` function
//! 5. Fetch the sign generator chunk

use std::time::Instant;

use anyhow::{Context, Result};
use regex::Regex;
use wreq::Client;

/// Base URL patterns for 17track's CDN.
const TRACKING_PAGE_URL: &str = "https://t.17track.net/en";

/// Fetched JS assets and page configuration.
#[derive(Clone, Debug)]
pub struct JsAssets {
    /// The sign generator module JS content (chunk 839 / ff19fa74, ~319KB).
    pub sign_module_js: String,
    /// The CDN base URL (e.g., `https://static.17track.net/t/2026-01/_next/static/chunks/`).
    pub base_url: String,
    /// The `window.YQ.configs.md5` value extracted from the page HTML.
    pub configs_md5: String,
    /// When these assets were fetched.
    pub fetched_at: Instant,
}

impl JsAssets {
    /// Check if cached assets are still fresh (1 hour TTL).
    pub fn is_fresh(&self) -> bool {
        self.fetched_at.elapsed() < std::time::Duration::from_secs(3600)
    }
}

/// Fetch JS assets from the 17track tracking page.
///
/// 1. Fetches the tracking page HTML to discover chunk URLs and configs.md5
/// 2. Fetches the webpack runtime to find chunk 839's filename
/// 3. Downloads the sign generator chunk
pub async fn fetch_js_assets(http_client: &Client) -> Result<JsAssets> {
    eprintln!("[js_fetcher] Fetching tracking page...");

    // Step 1: Fetch the tracking page HTML
    let html = http_client
        .get(TRACKING_PAGE_URL)
        .send()
        .await
        .context("Failed to fetch tracking page")?
        .text()
        .await
        .context("Failed to read tracking page body")?;

    eprintln!("[js_fetcher] Page fetched, {} bytes", html.len());

    // Step 2: Extract configs.md5 from inline script
    let configs_md5 = extract_configs_md5(&html).unwrap_or_else(|| "1.0.156".to_string());
    eprintln!("[js_fetcher] configs.md5 = {}", configs_md5);

    // Step 3: Find the CDN base URL from script references
    let base_url = extract_base_url(&html).context("Failed to find CDN base URL in HTML")?;
    eprintln!("[js_fetcher] CDN base: {}", base_url);

    // Step 4: Find and fetch the webpack runtime to get chunk mappings
    let webpack_runtime_url =
        find_webpack_runtime_url(&html).context("Failed to find webpack runtime URL in HTML")?;
    eprintln!("[js_fetcher] Webpack runtime: {}", webpack_runtime_url);

    let webpack_js = http_client
        .get(&webpack_runtime_url)
        .send()
        .await
        .context("Failed to fetch webpack runtime")?
        .text()
        .await
        .context("Failed to read webpack runtime body")?;

    eprintln!(
        "[js_fetcher] Webpack runtime fetched, {} bytes",
        webpack_js.len()
    );

    // Step 5: Extract chunk 839 URL from the webpack runtime
    let sign_chunk_url = find_sign_chunk_url_from_webpack(&webpack_js, &base_url)
        .context("Failed to find sign chunk URL in webpack runtime")?;
    eprintln!("[js_fetcher] Sign chunk URL: {}", sign_chunk_url);

    // Step 6: Fetch the sign module JS
    let sign_module_js = http_client
        .get(&sign_chunk_url)
        .send()
        .await
        .context("Failed to fetch sign module JS")?
        .text()
        .await
        .context("Failed to read sign module body")?;

    eprintln!(
        "[js_fetcher] Sign module fetched, {} bytes",
        sign_module_js.len()
    );

    Ok(JsAssets {
        sign_module_js,
        base_url,
        configs_md5,
        fetched_at: Instant::now(),
    })
}

/// Extract `window.YQ.configs.md5` from the page HTML.
fn extract_configs_md5(html: &str) -> Option<String> {
    let re = Regex::new(r#"configs\.md5\s*=\s*'([^']+)'"#).ok()?;
    re.captures(html)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

/// Extract the CDN base URL from script references in the HTML.
///
/// Looks for patterns like `https://static.17track.net/t/2026-01/_next/static/chunks/`
fn extract_base_url(html: &str) -> Option<String> {
    let re = Regex::new(r#"(https://static\.17track\.net/t/[^/]+/_next/static/chunks/)"#).ok()?;
    re.captures(html)
        .and_then(|cap| cap.get(1))
        .map(|m| m.as_str().to_string())
}

/// Find the webpack runtime URL from the HTML.
///
/// The App Router webpack runtime has `id="_R_"` on the script tag:
/// ```html
/// <script src="https://static.17track.net/.../webpack-{hash}.js" id="_R_" async="">
/// ```
fn find_webpack_runtime_url(html: &str) -> Option<String> {
    // Strategy 1: Look for script with id="_R_" (Next.js App Router marker)
    // The id and src can appear in either order in the tag
    let id_r_re = Regex::new(r#"<script[^>]*\bid="_R_"[^>]*\bsrc="([^"]+)"[^>]*>"#).ok()?;
    if let Some(cap) = id_r_re.captures(html)
        && let Some(url) = cap.get(1)
    {
        return Some(url.as_str().to_string());
    }

    // Also try with src before id
    let src_id_re = Regex::new(r#"<script[^>]*\bsrc="([^"]+)"[^>]*\bid="_R_"[^>]*>"#).ok()?;
    if let Some(cap) = src_id_re.captures(html)
        && let Some(url) = cap.get(1)
    {
        return Some(url.as_str().to_string());
    }

    // Strategy 2: Look for webpack-*.js in static.17track.net URLs
    let webpack_re =
        Regex::new(r#"(https://static\.17track\.net/[^"]*webpack-[a-f0-9]+\.js)"#).ok()?;
    if let Some(cap) = webpack_re.captures(html)
        && let Some(url) = cap.get(1)
    {
        return Some(url.as_str().to_string());
    }

    None
}

/// Extract the sign chunk URL from the webpack runtime JS.
///
/// The webpack runtime contains a `r.u` (or similar) function that maps chunk IDs
/// to filenames. For chunk 839, it produces `ff19fa74.{hash}.js`.
///
/// The pattern in the runtime looks like:
/// ```js
/// r.u = e => "static/chunks/" + ({211:"bb1bf137", 839:"ff19fa74"}[e] || e)
///     + "." + ({..., 839:"aac6e850586820c7"}[e]) + ".js"
/// ```
fn find_sign_chunk_url_from_webpack(webpack_js: &str, base_url: &str) -> Option<String> {
    // Strategy 1: Find both the name and hash mappings for chunk 839
    let name_re = Regex::new(r#"839:"([a-f0-9]{8})""#).ok()?;
    let hash_re = Regex::new(r#"839:"([a-f0-9]{16})""#).ok()?;

    if let (Some(name_cap), Some(hash_cap)) = (
        name_re
            .captures(webpack_js)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string())),
        hash_re
            .captures(webpack_js)
            .and_then(|c| c.get(1).map(|m| m.as_str().to_string())),
    ) {
        return Some(format!("{}{}.{}.js", base_url, name_cap, hash_cap));
    }

    // Strategy 2: Direct ff19fa74 pattern in webpack runtime
    let direct_re = Regex::new(r#"(ff19fa74\.[a-f0-9]+\.js)"#).ok()?;
    if let Some(cap) = direct_re.captures(webpack_js)
        && let Some(filename) = cap.get(1)
    {
        return Some(format!("{}{}", base_url, filename.as_str()));
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_configs_md5() {
        let html = r#"window.YQ.configs.md5 = '1.0.156'"#;
        assert_eq!(extract_configs_md5(html), Some("1.0.156".to_string()));

        let html2 = r#"configs.md5 = '2.0.0'"#;
        assert_eq!(extract_configs_md5(html2), Some("2.0.0".to_string()));

        assert_eq!(extract_configs_md5("no md5 here"), None);
    }

    #[test]
    fn test_extract_base_url() {
        let html = r#"src="https://static.17track.net/t/2026-01/_next/static/chunks/119-22a90af49d5bd9ee.js""#;
        assert_eq!(
            extract_base_url(html),
            Some("https://static.17track.net/t/2026-01/_next/static/chunks/".to_string())
        );
    }

    #[test]
    fn test_find_webpack_runtime_url_id_r() {
        let html = r#"<script src="https://static.17track.net/t/2026-01/_next/static/chunks/webpack-49544beacf8ff63a.js" id="_R_" async=""></script>"#;
        assert_eq!(
            find_webpack_runtime_url(html),
            Some("https://static.17track.net/t/2026-01/_next/static/chunks/webpack-49544beacf8ff63a.js".to_string())
        );
    }

    #[test]
    fn test_find_webpack_runtime_url_fallback() {
        let html = r#"<script src="https://static.17track.net/t/2026-01/_next/static/chunks/webpack-abc123def456.js" async></script>"#;
        assert_eq!(
            find_webpack_runtime_url(html),
            Some(
                "https://static.17track.net/t/2026-01/_next/static/chunks/webpack-abc123def456.js"
                    .to_string()
            )
        );
    }

    #[test]
    fn test_find_sign_chunk_from_webpack() {
        let webpack_js = r#"r.u=e=>"static/chunks/"+(({211:"bb1bf137",839:"ff19fa74"})[e]||e)+"."+(({32:"8516d9b556cf70fb",51:"b290a4f7e71aa4ad",166:"2cb66e73ed45f29c",211:"6b2d4eab87f959da",839:"aac6e850586820c7"})[e])+".js""#;
        let base = "https://static.17track.net/t/2026-01/_next/static/chunks/";
        let url = find_sign_chunk_url_from_webpack(webpack_js, base);
        assert_eq!(url, Some(format!("{}ff19fa74.aac6e850586820c7.js", base)));
    }

    #[test]
    fn test_find_sign_chunk_direct_fallback() {
        let webpack_js = r#"something ff19fa74.aac6e850586820c7.js something"#;
        let base = "https://static.17track.net/t/2026-01/_next/static/chunks/";
        assert_eq!(
            find_sign_chunk_url_from_webpack(webpack_js, base),
            Some(format!("{}ff19fa74.aac6e850586820c7.js", base))
        );
    }
}

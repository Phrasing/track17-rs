/// Look up city and state from a US zip code
pub fn lookup_zipcode(zip: &str) -> Option<(String, String)> {
    // Avoid zipcodes::matching to suppress debug_print output.
    let results = zipcodes::filter_by(vec![|z| z.zip_code == zip], None).ok()?;
    let info = results.first()?;
    Some((info.city.clone(), info.state.clone()))
}

/// Format a location string, resolving US zip codes to city/state
pub fn format_location(raw: &str) -> String {
    // Try to parse "US ZIPCODE" format
    let parts: Vec<&str> = raw.split_whitespace().collect();
    if parts.len() == 2 && parts[0] == "US" {
        let zip = parts[1];
        if let Some((city, state)) = lookup_zipcode(zip) {
            return format!("{}, {}", city, state);
        }
    }
    // Fall back to raw location
    raw.to_string()
}

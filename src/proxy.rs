/// Parsed proxy configuration
#[derive(Debug, Clone)]
pub struct ProxyConfig {
    pub scheme: String,
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
}

impl ProxyConfig {
    /// Parse proxy string in various formats:
    /// - http://host:port:user:pass
    /// - host:port:user:pass
    /// - user:pass@host:port
    /// - user:pass:host:port
    /// - http://user:pass@host:port
    /// - http://user:pass:host:port
    /// - https://user:pass@host:port
    pub fn parse(proxy: &str) -> Option<Self> {
        let proxy = proxy.trim();
        if proxy.is_empty() {
            return None;
        }

        // Extract scheme if present
        let (scheme, rest) = if proxy.starts_with("https://") {
            ("https".to_string(), &proxy[8..])
        } else if proxy.starts_with("http://") {
            ("http".to_string(), &proxy[7..])
        } else {
            ("http".to_string(), proxy)
        };

        // Try standard URL format: user:pass@host:port
        if let Some(at_pos) = rest.rfind('@') {
            let auth = &rest[..at_pos];
            let host_port = &rest[at_pos + 1..];

            let (host, port) = parse_host_port(host_port)?;
            let (username, password) = parse_user_pass_colon(auth);

            return Some(ProxyConfig {
                scheme,
                host,
                port,
                username: Some(username),
                password: Some(password),
            });
        }

        // Try colon-separated formats
        let parts: Vec<&str> = rest.split(':').collect();

        match parts.len() {
            // host:port
            2 => {
                let host = parts[0].to_string();
                let port = parts[1].parse().ok()?;
                Some(ProxyConfig {
                    scheme,
                    host,
                    port,
                    username: None,
                    password: None,
                })
            }
            // Could be host:port:user:pass or user:pass:host:port
            4 => {
                // Try host:port:user:pass first (port should be numeric)
                if let Ok(port) = parts[1].parse::<u16>() {
                    Some(ProxyConfig {
                        scheme,
                        host: parts[0].to_string(),
                        port,
                        username: Some(parts[2].to_string()),
                        password: Some(parts[3].to_string()),
                    })
                }
                // Try user:pass:host:port (last part should be numeric port)
                else if let Ok(port) = parts[3].parse::<u16>() {
                    Some(ProxyConfig {
                        scheme,
                        host: parts[2].to_string(),
                        port,
                        username: Some(parts[0].to_string()),
                        password: Some(parts[1].to_string()),
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    /// Get the proxy URL for wreq (http://user:pass@host:port)
    pub fn to_url(&self) -> String {
        match (&self.username, &self.password) {
            (Some(user), Some(pass)) => {
                format!("{}://{}:{}@{}:{}", self.scheme, user, pass, self.host, self.port)
            }
            _ => format!("{}://{}:{}", self.scheme, self.host, self.port),
        }
    }

    /// Get the proxy URL without auth for browser (host:port)
    pub fn to_host_port(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

fn parse_host_port(s: &str) -> Option<(String, u16)> {
    let parts: Vec<&str> = s.split(':').collect();
    if parts.len() == 2 {
        let host = parts[0].to_string();
        let port = parts[1].parse().ok()?;
        Some((host, port))
    } else {
        None
    }
}

fn parse_user_pass_colon(s: &str) -> (String, String) {
    // Find the first colon to split user:pass
    if let Some(colon_pos) = s.find(':') {
        let user = s[..colon_pos].to_string();
        let pass = s[colon_pos + 1..].to_string();
        (user, pass)
    } else {
        (s.to_string(), String::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_formats() {
        // Format 1: http://host:port:user:pass
        let p = ProxyConfig::parse("http://proxy.example.com:8080:user:pass123").unwrap();
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, 8080);
        assert_eq!(p.username.as_deref(), Some("user"));
        assert_eq!(p.password.as_deref(), Some("pass123"));

        // Format 2: host:port:user:pass
        let p = ProxyConfig::parse("proxy.example.com:8080:user:pass123").unwrap();
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, 8080);

        // Format 3: user:pass@host:port
        let p = ProxyConfig::parse("user:pass123@proxy.example.com:8080").unwrap();
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, 8080);
        assert_eq!(p.username.as_deref(), Some("user"));

        // Format 4: http://user:pass@host:port
        let p = ProxyConfig::parse("http://user:pass123@proxy.example.com:8080").unwrap();
        assert_eq!(p.scheme, "http");
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, 8080);

        // Format 5: https://user:pass@host:port
        let p = ProxyConfig::parse("https://user:pass123@proxy.example.com:8443").unwrap();
        assert_eq!(p.scheme, "https");
        assert_eq!(p.host, "proxy.example.com");
        assert_eq!(p.port, 8443);
    }
}

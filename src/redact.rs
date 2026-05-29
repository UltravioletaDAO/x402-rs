//! Stream-safe redaction helpers for URLs and secret-bearing strings.
//!
//! All facilitator log output may be viewed live by the user on stream
//! (per global policy). Any value passing through `tracing` macros that could
//! contain an API key, private key, or token MUST be redacted here first.

/// Redact an RPC URL by stripping path, query and fragment.
///
/// Many provider URLs carry the API key in the path (QuickNode, Alchemy) or
/// the query string (Infura). Returning only `scheme://host[:port]` keeps the
/// information useful for ops (you can still see which provider it is) while
/// removing the credential portion.
///
/// Falls back to the literal string `"<redacted-rpc>"` if parsing fails — we
/// would rather lose context than risk leaking a malformed but still
/// sensitive URL.
pub fn rpc_url(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(parsed) => {
            let scheme = parsed.scheme();
            let Some(host) = parsed.host_str() else {
                return "<redacted-rpc>".to_string();
            };
            match parsed.port() {
                Some(port) => format!("{scheme}://{host}:{port}/<redacted>"),
                None => format!("{scheme}://{host}/<redacted>"),
            }
        }
        Err(_) => "<redacted-rpc>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_quicknode_path_api_key() {
        let raw = "https://node-name.arbitrum-mainnet.quiknode.pro/abcdef1234567890/";
        assert_eq!(
            rpc_url(raw),
            "https://node-name.arbitrum-mainnet.quiknode.pro/<redacted>"
        );
    }

    #[test]
    fn strips_infura_query_api_key() {
        let raw = "https://mainnet.infura.io/v3/0123456789abcdef0123456789abcdef";
        assert_eq!(rpc_url(raw), "https://mainnet.infura.io/<redacted>");
    }

    #[test]
    fn preserves_explicit_port() {
        let raw = "http://localhost:8545/some/path";
        assert_eq!(rpc_url(raw), "http://localhost:8545/<redacted>");
    }

    #[test]
    fn returns_placeholder_on_garbage() {
        assert_eq!(rpc_url("not a url"), "<redacted-rpc>");
        assert_eq!(rpc_url(""), "<redacted-rpc>");
    }
}

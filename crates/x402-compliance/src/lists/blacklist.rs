use crate::error::{ComplianceError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlacklistEntry {
    pub account_type: String,
    pub wallet: String,
    pub reason: String,
}

/// Custom blacklist for manual address blocking
#[derive(Debug, Clone)]
pub struct Blacklist {
    addresses: HashSet<String>,
    entries: Vec<BlacklistEntry>,
}

impl Blacklist {
    pub fn from_file<P: AsRef<Path>>(path: P) -> Result<Self> {
        let content = fs::read_to_string(path.as_ref()).map_err(|e| {
            ComplianceError::ListLoadError(format!("Failed to read blacklist file: {}", e))
        })?;

        Self::from_string(&content)
    }

    pub fn from_string(content: &str) -> Result<Self> {
        let entries: Vec<BlacklistEntry> = serde_json::from_str(content)
            .map_err(|e| ComplianceError::ListLoadError(format!("Failed to parse blacklist JSON: {}", e)))?;

        let mut addresses = HashSet::new();

        for entry in &entries {
            let normalized_wallet = entry.wallet.to_lowercase().trim().to_string();
            if !normalized_wallet.is_empty() {
                addresses.insert(normalized_wallet);
            }
        }

        tracing::info!("Loaded blacklist: {} addresses", addresses.len());

        Ok(Self { addresses, entries })
    }

    pub fn empty() -> Self {
        Self {
            addresses: HashSet::new(),
            entries: Vec::new(),
        }
    }

    pub fn is_blacklisted(&self, address: &str) -> bool {
        let normalized = address.to_lowercase().trim().to_string();
        self.addresses.contains(&normalized)
    }

    pub fn get_reason(&self, address: &str) -> Option<String> {
        let normalized = address.to_lowercase().trim().to_string();
        if self.addresses.contains(&normalized) {
            self.entries
                .iter()
                .find(|e| e.wallet.to_lowercase() == normalized)
                .map(|e| e.reason.clone())
        } else {
            None
        }
    }

    pub fn total_blocked(&self) -> usize {
        self.addresses.len()
    }

    pub fn entries(&self) -> &[BlacklistEntry] {
        &self.entries
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_blacklist() {
        let blacklist = Blacklist::empty();
        assert_eq!(blacklist.total_blocked(), 0);
        assert!(!blacklist.is_blacklisted("0x1234"));
    }

    #[test]
    fn test_load_from_string() {
        let json = r#"[
            {
                "account_type": "evm",
                "wallet": "0x1234567890123456789012345678901234567890",
                "reason": "spam"
            },
            {
                "account_type": "solana",
                "wallet": "ABC123",
                "reason": "test"
            }
        ]"#;

        let blacklist = Blacklist::from_string(json).unwrap();
        assert_eq!(blacklist.total_blocked(), 2);
        assert!(blacklist.is_blacklisted("0x1234567890123456789012345678901234567890"));
        assert!(blacklist.is_blacklisted("ABC123"));
    }

    #[test]
    fn test_case_insensitive() {
        let json = r#"[
            {
                "account_type": "evm",
                "wallet": "0xABCDEF",
                "reason": "test"
            }
        ]"#;

        let blacklist = Blacklist::from_string(json).unwrap();
        assert!(blacklist.is_blacklisted("0xabcdef"));
        assert!(blacklist.is_blacklisted("0xABCDEF"));
        assert!(blacklist.is_blacklisted("0xAbCdEf"));
    }

    #[test]
    fn test_reason_retrieval() {
        let json = r#"[
            {
                "account_type": "solana",
                "wallet": "TestWallet123",
                "reason": "spam account"
            }
        ]"#;

        let blacklist = Blacklist::from_string(json).unwrap();
        let reason = blacklist.get_reason("TestWallet123");
        assert_eq!(reason, Some("spam account".to_string()));
    }
}

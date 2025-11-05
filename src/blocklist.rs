use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use std::sync::Arc;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BlacklistEntry {
    pub account_type: String,
    pub wallet: String,
    pub reason: String,
}

#[derive(Debug, Clone)]
pub struct Blacklist {
    evm_addresses: HashSet<String>,
    solana_addresses: HashSet<String>,
    entries: Vec<BlacklistEntry>,
}

impl Blacklist {
    pub fn load_from_file<P: AsRef<Path>>(path: P) -> Result<Self, BlacklistError> {
        let content = fs::read_to_string(path)
            .map_err(|e| BlacklistError::FileReadError(format!("Failed to read blacklist file: {}", e)))?;

        Self::load_from_string(&content)
    }

    pub fn load_from_string(content: &str) -> Result<Self, BlacklistError> {
        let entries: Vec<BlacklistEntry> = serde_json::from_str(content)
            .map_err(|e| BlacklistError::ParseError(format!("Failed to parse blacklist JSON: {}", e)))?;

        let mut evm_addresses = HashSet::new();
        let mut solana_addresses = HashSet::new();

        for entry in &entries {
            let normalized_wallet = entry.wallet.to_lowercase().trim().to_string();

            match entry.account_type.to_lowercase().as_str() {
                "evm" => {
                    if !normalized_wallet.is_empty() {
                        evm_addresses.insert(normalized_wallet);
                    }
                }
                "solana" => {
                    if !normalized_wallet.is_empty() {
                        solana_addresses.insert(normalized_wallet);
                    }
                }
                _ => {
                    tracing::warn!(
                        "Unknown account_type '{}' in blacklist entry for wallet '{}'",
                        entry.account_type,
                        entry.wallet
                    );
                }
            }
        }

        tracing::info!(
            "Loaded blacklist: {} EVM addresses, {} Solana addresses",
            evm_addresses.len(),
            solana_addresses.len()
        );

        Ok(Self {
            evm_addresses,
            solana_addresses,
            entries,
        })
    }

    pub fn empty() -> Self {
        Self {
            evm_addresses: HashSet::new(),
            solana_addresses: HashSet::new(),
            entries: Vec::new(),
        }
    }

    pub fn is_evm_blocked(&self, address: &str) -> Option<String> {
        let normalized = address.to_lowercase().trim().to_string();
        if self.evm_addresses.contains(&normalized) {
            self.entries
                .iter()
                .find(|e| e.account_type.to_lowercase() == "evm" && e.wallet.to_lowercase() == normalized)
                .map(|e| e.reason.clone())
        } else {
            None
        }
    }

    pub fn is_solana_blocked(&self, address: &str) -> Option<String> {
        let normalized = address.to_lowercase().trim().to_string();
        if self.solana_addresses.contains(&normalized) {
            self.entries
                .iter()
                .find(|e| e.account_type.to_lowercase() == "solana" && e.wallet.to_lowercase() == normalized)
                .map(|e| e.reason.clone())
        } else {
            None
        }
    }

    pub fn total_blocked(&self) -> usize {
        self.evm_addresses.len() + self.solana_addresses.len()
    }

    pub fn evm_count(&self) -> usize {
        self.evm_addresses.len()
    }

    pub fn solana_count(&self) -> usize {
        self.solana_addresses.len()
    }

    pub fn entries(&self) -> &[BlacklistEntry] {
        &self.entries
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BlacklistError {
    #[error("File read error: {0}")]
    FileReadError(String),
    #[error("Parse error: {0}")]
    ParseError(String),
}

pub type SharedBlacklist = Arc<Blacklist>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_empty_blacklist() {
        let blacklist = Blacklist::empty();
        assert_eq!(blacklist.total_blocked(), 0);
        assert!(blacklist.is_evm_blocked("0x1234").is_none());
        assert!(blacklist.is_solana_blocked("ABC123").is_none());
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

        let blacklist = Blacklist::load_from_string(json).unwrap();
        assert_eq!(blacklist.total_blocked(), 2);
        assert!(blacklist.is_evm_blocked("0x1234567890123456789012345678901234567890").is_some());
        assert!(blacklist.is_solana_blocked("ABC123").is_some());
    }

    #[test]
    fn test_case_insensitive() {
        let json = r#"[
            {
                "account_type": "EVM",
                "wallet": "0xABCDEF",
                "reason": "test"
            }
        ]"#;

        let blacklist = Blacklist::load_from_string(json).unwrap();
        assert!(blacklist.is_evm_blocked("0xabcdef").is_some());
        assert!(blacklist.is_evm_blocked("0xABCDEF").is_some());
        assert!(blacklist.is_evm_blocked("0xAbCdEf").is_some());
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

        let blacklist = Blacklist::load_from_string(json).unwrap();
        let reason = blacklist.is_solana_blocked("TestWallet123");
        assert_eq!(reason, Some("spam account".to_string()));
    }
}

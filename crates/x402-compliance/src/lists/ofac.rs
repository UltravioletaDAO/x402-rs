use crate::checker::ListMetadata;
use crate::config::ListConfig;
use crate::error::{ComplianceError, Result};
use crate::lists::SanctionsList;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;

/// Metadata about the OFAC sanctions list
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfacMetadata {
    /// Source description
    pub source: String,
    /// URL where the list was downloaded from
    pub source_url: String,
    /// ISO 8601 timestamp when the list was generated
    pub generated_at: String,
    /// Total number of addresses in the list
    pub total_addresses: usize,
    /// List of supported blockchain currencies
    pub currencies: Vec<String>,
}

/// A single sanctioned address entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfacAddress {
    /// The cryptocurrency address (normalized to lowercase)
    pub address: String,
    /// The blockchain/currency type (e.g., "ethereum", "bitcoin", "solana")
    pub blockchain: String,
    /// Name of the sanctioned entity
    pub entity_name: String,
    /// OFAC entity ID
    pub entity_id: String,
    /// Reason for sanctions
    pub reason: String,
}

/// Root structure of the OFAC addresses JSON file
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OfacData {
    /// Metadata about the list
    pub metadata: OfacMetadata,
    /// List of all sanctioned addresses
    pub addresses: Vec<OfacAddress>,
}

/// OFAC sanctions list implementation
#[derive(Debug, Clone)]
pub struct OfacList {
    /// Set of sanctioned addresses (normalized to lowercase)
    sanctioned_addresses: HashSet<String>,
    /// Full address data with entity information
    address_data: Vec<OfacAddress>,
    /// Metadata about the loaded list
    metadata: OfacMetadata,
    /// SHA-256 checksum of the loaded file
    checksum: String,
    /// Last updated timestamp
    last_updated: Option<chrono::DateTime<chrono::Utc>>,
}

impl OfacList {
    /// Load OFAC sanctions list from configuration
    pub async fn load(config: &ListConfig) -> Result<Self> {
        tracing::info!("Loading OFAC sanctions list from: {}", config.path.display());

        // Read the JSON file
        let content = fs::read_to_string(&config.path).map_err(|e| {
            ComplianceError::ListLoadError(format!(
                "Failed to read OFAC file {}: {}",
                config.path.display(),
                e
            ))
        })?;

        // Calculate SHA-256 checksum
        let mut hasher = Sha256::new();
        hasher.update(content.as_bytes());
        let checksum = format!("{:x}", hasher.finalize());

        // Parse JSON
        let data: OfacData = serde_json::from_str(&content).map_err(|e| {
            ComplianceError::ListLoadError(format!("Failed to parse OFAC JSON: {}", e))
        })?;

        // Build HashSet for fast lookups
        let sanctioned_addresses: HashSet<String> = data
            .addresses
            .iter()
            .map(|addr| addr.address.to_lowercase())
            .collect();

        // Get file metadata for last_updated
        let last_updated = fs::metadata(&config.path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|time| chrono::DateTime::<chrono::Utc>::from(time));

        tracing::info!(
            "Loaded OFAC list: {} addresses across {} currencies (generated: {})",
            data.metadata.total_addresses,
            data.metadata.currencies.len(),
            data.metadata.generated_at
        );

        tracing::debug!("Supported currencies: {:?}", data.metadata.currencies);
        tracing::debug!("List checksum: {}", checksum);

        Ok(Self {
            sanctioned_addresses,
            address_data: data.addresses,
            metadata: data.metadata,
            checksum,
            last_updated,
        })
    }

    /// Get entity information for a sanctioned address
    pub fn get_entity_info(&self, address: &str) -> Option<&OfacAddress> {
        let normalized = address.to_lowercase();
        self.address_data
            .iter()
            .find(|addr| addr.address.to_lowercase() == normalized)
    }
}

impl SanctionsList for OfacList {
    fn is_sanctioned(&self, address: &str) -> bool {
        let normalized = address.to_lowercase();
        let is_sanctioned = self.sanctioned_addresses.contains(&normalized);

        if is_sanctioned {
            tracing::warn!("OFAC ALERT: Sanctioned address detected: {}", address);
        }

        is_sanctioned
    }

    fn metadata(&self) -> ListMetadata {
        ListMetadata {
            name: "OFAC_SDN".to_string(),
            enabled: true,
            record_count: self.sanctioned_addresses.len(),
            last_updated: self.last_updated,
            checksum: Some(self.checksum.clone()),
            source_url: self.metadata.source_url.clone(),
        }
    }

    fn total_addresses(&self) -> usize {
        self.sanctioned_addresses.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn create_test_ofac_file() -> NamedTempFile {
        let mut file = NamedTempFile::new().unwrap();

        let test_data = r#"{
  "metadata": {
    "source": "Test OFAC List",
    "source_url": "https://example.com",
    "generated_at": "2025-11-10T00:00:00Z",
    "total_addresses": 3,
    "currencies": ["ethereum", "bitcoin"]
  },
  "addresses": [
    {
      "address": "0x1234567890123456789012345678901234567890",
      "blockchain": "ethereum",
      "entity_name": "Test Entity 1",
      "entity_id": "1",
      "reason": "OFAC SDN List"
    },
    {
      "address": "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd",
      "blockchain": "ethereum",
      "entity_name": "Test Entity 2",
      "entity_id": "2",
      "reason": "OFAC SDN List"
    },
    {
      "address": "1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2",
      "blockchain": "bitcoin",
      "entity_name": "Test Entity 3",
      "entity_id": "3",
      "reason": "OFAC SDN List"
    }
  ]
}"#;

        file.write_all(test_data.as_bytes()).unwrap();
        file.flush().unwrap();
        file
    }

    #[tokio::test]
    async fn test_load_from_file() {
        let file = create_test_ofac_file();
        let config = ListConfig {
            enabled: true,
            path: file.path().to_path_buf(),
            source_url: None,
            auto_update: false,
            update_interval_hours: 24,
        };

        let list = OfacList::load(&config).await.unwrap();
        assert_eq!(list.total_addresses(), 3);
        assert_eq!(list.metadata().record_count, 3);
    }

    #[tokio::test]
    async fn test_sanctioned_address_detection() {
        let file = create_test_ofac_file();
        let config = ListConfig {
            enabled: true,
            path: file.path().to_path_buf(),
            source_url: None,
            auto_update: false,
            update_interval_hours: 24,
        };

        let list = OfacList::load(&config).await.unwrap();

        // Test exact match (lowercase)
        assert!(list.is_sanctioned("0x1234567890123456789012345678901234567890"));

        // Test case insensitivity
        assert!(list.is_sanctioned("0X1234567890123456789012345678901234567890"));
        assert!(list.is_sanctioned("0xAbCdEfAbCdEfAbCdEfAbCdEfAbCdEfAbCdEfAbCd"));

        // Test Bitcoin address
        assert!(list.is_sanctioned("1BvBMSEYstWetqTFn5Au4m4GFg7xJaNVN2"));

        // Test non-sanctioned address
        assert!(!list.is_sanctioned("0x9999999999999999999999999999999999999999"));
    }

    #[tokio::test]
    async fn test_entity_info() {
        let file = create_test_ofac_file();
        let config = ListConfig {
            enabled: true,
            path: file.path().to_path_buf(),
            source_url: None,
            auto_update: false,
            update_interval_hours: 24,
        };

        let list = OfacList::load(&config).await.unwrap();

        let info = list.get_entity_info("0x1234567890123456789012345678901234567890");
        assert!(info.is_some());
        assert_eq!(info.unwrap().entity_name, "Test Entity 1");
    }
}

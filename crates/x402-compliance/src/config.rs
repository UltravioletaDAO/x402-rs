use crate::error::{ComplianceError, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub lists: Lists,
    pub blacklist_path: Option<PathBuf>,
    pub audit_logging: AuditLoggingConfig,
    pub fail_mode: FailMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Lists {
    pub ofac: ListConfig,
    #[cfg(feature = "un")]
    pub un: ListConfig,
    #[cfg(feature = "uk")]
    pub uk: ListConfig,
    #[cfg(feature = "eu")]
    pub eu: ListConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListConfig {
    pub enabled: bool,
    pub path: PathBuf,
    pub source_url: Option<String>,
    pub auto_update: bool,
    #[serde(default = "default_update_interval")]
    pub update_interval_hours: u64,
}

fn default_update_interval() -> u64 {
    24
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditLoggingConfig {
    pub enabled: bool,
    pub target: String,
    pub format: LogFormat,
    pub include_clear_transactions: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum LogFormat {
    Json,
    Text,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FailMode {
    pub on_list_load_error: FailModeType,
    pub on_screening_error: FailModeType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FailModeType {
    Open,   // Continue without screening if error
    Closed, // Block all transactions if error
}

impl Config {
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let content = std::fs::read_to_string(&path)
            .map_err(|e| ComplianceError::ConfigError(format!("Failed to read config file: {}", e)))?;

        toml::from_str(&content).map_err(ComplianceError::TomlError)
    }

    pub fn from_env() -> Result<Self> {
        // Try to load from default locations
        let default_paths = [
            "config/compliance.toml",
            "compliance.toml",
            ".compliance.toml",
        ];

        for path in &default_paths {
            if std::path::Path::new(path).exists() {
                return Self::from_file(path);
            }
        }

        // If no config file found, use defaults
        Ok(Self::default())
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            lists: Lists {
                ofac: ListConfig {
                    enabled: true,
                    path: PathBuf::from("config/ofac_addresses.json"),
                    source_url: Some("https://sanctionslistservice.ofac.treas.gov/api/PublicationPreview/exports/ADVANCED.JSON".to_string()),
                    auto_update: false,
                    update_interval_hours: 24,
                },
                #[cfg(feature = "un")]
                un: ListConfig {
                    enabled: false,
                    path: PathBuf::from("config/un_consolidated.json"),
                    source_url: Some("https://www.un.org/securitycouncil/content/un-sc-consolidated-list".to_string()),
                    auto_update: false,
                    update_interval_hours: 168, // Weekly
                },
                #[cfg(feature = "uk")]
                uk: ListConfig {
                    enabled: false,
                    path: PathBuf::from("config/uk_ofsi.json"),
                    source_url: Some("https://www.gov.uk/government/publications/financial-sanctions-consolidated-list-of-targets".to_string()),
                    auto_update: false,
                    update_interval_hours: 24,
                },
                #[cfg(feature = "eu")]
                eu: ListConfig {
                    enabled: false,
                    path: PathBuf::from("config/eu_sanctions.json"),
                    source_url: Some("https://data.europa.eu/data/datasets/consolidated-list-of-persons-groups-and-entities-subject-to-eu-financial-sanctions".to_string()),
                    auto_update: false,
                    update_interval_hours: 24,
                },
            },
            blacklist_path: Some(PathBuf::from("config/blacklist.json")),
            audit_logging: AuditLoggingConfig {
                enabled: true,
                target: "compliance_audit".to_string(),
                format: LogFormat::Json,
                include_clear_transactions: false,
            },
            fail_mode: FailMode {
                on_list_load_error: FailModeType::Open,
                on_screening_error: FailModeType::Open,
            },
        }
    }
}

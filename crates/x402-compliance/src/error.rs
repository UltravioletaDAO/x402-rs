use thiserror::Error;

#[derive(Error, Debug)]
pub enum ComplianceError {
    #[error("Failed to load sanctions list: {0}")]
    ListLoadError(String),

    #[error("Failed to extract addresses: {0}")]
    AddressExtraction(String),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("IO error: {0}")]
    IoError(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    SerdeError(#[from] serde_json::Error),

    #[error("TOML parsing error: {0}")]
    TomlError(#[from] toml::de::Error),

    #[error("Invalid checksum")]
    InvalidChecksum,

    #[cfg(feature = "solana")]
    #[error("Solana transaction parsing error: {0}")]
    SolanaError(String),
}

pub type Result<T> = std::result::Result<T, ComplianceError>;

pub mod checker;
pub mod error;
pub mod lists;
pub mod extractors;
pub mod audit_logger;
pub mod config;

// Re-export main types for convenience
pub use checker::{
    ComplianceChecker, ComplianceCheckerBuilder, ScreeningResult, ScreeningDecision,
    TransactionContext, AddressType, MatchedEntity,
};
pub use error::{ComplianceError, Result};
pub use audit_logger::{AuditLogger, ComplianceEvent, EventType, Decision};
pub use config::{Config, ListConfig};

// Re-export extractors
pub use extractors::evm::EvmExtractor;
#[cfg(feature = "solana")]
pub use extractors::solana::SolanaExtractor;

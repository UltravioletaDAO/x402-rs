use crate::error::Result;
use crate::lists::SanctionsList;
use crate::audit_logger::{AuditLogger, ComplianceEvent, EventType, Decision};
use crate::config::Config;
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// Main trait for compliance screening
#[async_trait]
pub trait ComplianceChecker: Send + Sync {
    /// Screen a payment transaction
    async fn screen_payment(
        &self,
        payer: &str,
        payee: &str,
        context: &TransactionContext,
    ) -> Result<ScreeningResult>;

    /// Screen a single address
    async fn screen_address(&self, address: &str) -> Result<ScreeningDecision>;

    /// Check if a specific list is loaded
    fn is_list_enabled(&self, list_name: &str) -> bool;

    /// Get metadata about loaded lists
    fn list_metadata(&self) -> HashMap<String, ListMetadata>;

    /// Reload/refresh sanctions lists
    async fn reload_lists(&mut self) -> Result<()>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScreeningResult {
    pub decision: ScreeningDecision,
    pub payer_address: String,
    pub payee_address: String,
    pub matched_entities: Vec<MatchedEntity>,
    pub list_versions: HashMap<String, String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ScreeningDecision {
    /// Transaction should be blocked
    Block { reason: String },
    /// Transaction needs manual review
    Review { reason: String },
    /// Transaction is clear to proceed
    Clear,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MatchedEntity {
    pub address: String,
    pub address_type: AddressType,
    pub list_source: String,
    pub entity_name: Option<String>,
    pub entity_id: Option<String>,
    pub program: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AddressType {
    Payer,
    Payee,
}

impl std::fmt::Display for AddressType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AddressType::Payer => write!(f, "payer"),
            AddressType::Payee => write!(f, "payee"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TransactionContext {
    pub amount: String,
    pub currency: String,
    pub network: String,
    pub transaction_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ListMetadata {
    pub name: String,
    pub enabled: bool,
    pub record_count: usize,
    pub last_updated: Option<chrono::DateTime<chrono::Utc>>,
    pub checksum: Option<String>,
    pub source_url: String,
}

/// Builder for creating ComplianceChecker instances
pub struct ComplianceCheckerBuilder {
    ofac_enabled: bool,
    un_enabled: bool,
    uk_enabled: bool,
    eu_enabled: bool,
    blacklist_path: Option<std::path::PathBuf>,
    config_path: Option<std::path::PathBuf>,
    audit_logger: Option<Arc<AuditLogger>>,
}

impl ComplianceCheckerBuilder {
    pub fn new() -> Self {
        Self {
            ofac_enabled: true,
            un_enabled: false,
            uk_enabled: false,
            eu_enabled: false,
            blacklist_path: None,
            config_path: None,
            audit_logger: None,
        }
    }

    pub fn with_ofac(mut self, enabled: bool) -> Self {
        self.ofac_enabled = enabled;
        self
    }

    pub fn with_un(mut self, enabled: bool) -> Self {
        self.un_enabled = enabled;
        self
    }

    pub fn with_uk(mut self, enabled: bool) -> Self {
        self.uk_enabled = enabled;
        self
    }

    pub fn with_eu(mut self, enabled: bool) -> Self {
        self.eu_enabled = enabled;
        self
    }

    pub fn with_blacklist(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.blacklist_path = Some(path.into());
        self
    }

    pub fn with_config_file(mut self, path: impl Into<std::path::PathBuf>) -> Self {
        self.config_path = Some(path.into());
        self
    }

    pub fn with_audit_logger(mut self, logger: Arc<AuditLogger>) -> Self {
        self.audit_logger = Some(logger);
        self
    }

    pub async fn build(self) -> Result<Box<dyn ComplianceChecker>> {
        // Load config if provided
        let config = if let Some(path) = self.config_path {
            Config::from_file(path)?
        } else {
            Config::default()
        };

        // Load sanctions lists
        let mut lists: Vec<Box<dyn SanctionsList>> = Vec::new();

        if self.ofac_enabled {
            let ofac = crate::lists::ofac::OfacList::load(&config.lists.ofac).await?;
            lists.push(Box::new(ofac));
        }

        // TODO: Add UN, UK, EU lists in Phase 2

        // Load blacklist if provided
        let blacklist = if let Some(path) = &self.blacklist_path {
            Some(crate::lists::blacklist::Blacklist::from_file(path)?)
        } else if let Some(path) = &config.blacklist_path {
            Some(crate::lists::blacklist::Blacklist::from_file(path)?)
        } else {
            None
        };

        // Create audit logger
        let audit_logger = self.audit_logger.unwrap_or_else(|| {
            Arc::new(AuditLogger::new(config.audit_logging.clone()))
        });

        Ok(Box::new(MultiListChecker {
            lists,
            blacklist,
            audit_logger,
            config,
        }))
    }
}

impl Default for ComplianceCheckerBuilder {
    fn default() -> Self {
        Self::new()
    }
}

/// Implementation of ComplianceChecker that checks multiple lists
pub struct MultiListChecker {
    lists: Vec<Box<dyn SanctionsList>>,
    blacklist: Option<crate::lists::blacklist::Blacklist>,
    audit_logger: Arc<AuditLogger>,
    config: Config,
}

#[async_trait]
impl ComplianceChecker for MultiListChecker {
    async fn screen_payment(
        &self,
        payer: &str,
        payee: &str,
        context: &TransactionContext,
    ) -> Result<ScreeningResult> {
        let mut matched_entities = Vec::new();
        let mut list_versions = HashMap::new();

        // Screen both payer and payee
        for (address, address_type) in [
            (payer, AddressType::Payer),
            (payee, AddressType::Payee),
        ] {
            // Check blacklist first
            if let Some(blacklist) = &self.blacklist {
                if blacklist.is_blacklisted(address) {
                    let matched = MatchedEntity {
                        address: address.to_string(),
                        address_type: address_type.clone(),
                        list_source: "blacklist".to_string(),
                        entity_name: None,
                        entity_id: None,
                        program: None,
                    };

                    matched_entities.push(matched);

                    // Log compliance event
                    self.audit_logger.log_event(ComplianceEvent {
                        timestamp: chrono::Utc::now(),
                        event_type: EventType::BlacklistHit,
                        decision: Decision::Block,
                        transaction_context: context.clone(),
                        matched_address: address.to_string(),
                        address_type: address_type.clone(),
                        list_source: "blacklist".to_string(),
                        entity_name: None,
                    });

                    return Ok(ScreeningResult {
                        decision: ScreeningDecision::Block {
                            reason: format!("Address is blacklisted ({})", address_type),
                        },
                        payer_address: payer.to_string(),
                        payee_address: payee.to_string(),
                        matched_entities,
                        list_versions,
                    });
                }
            }

            // Check sanctions lists
            for list in &self.lists {
                if list.is_sanctioned(address) {
                    let metadata = list.metadata();
                    list_versions.insert(metadata.name.clone(), metadata.checksum.clone().unwrap_or_default());

                    let matched = MatchedEntity {
                        address: address.to_string(),
                        address_type: address_type.clone(),
                        list_source: metadata.name.clone(),
                        entity_name: None, // TODO: Extract entity name in Phase 2
                        entity_id: None,
                        program: None,
                    };

                    matched_entities.push(matched);

                    // Log compliance event
                    self.audit_logger.log_event(ComplianceEvent {
                        timestamp: chrono::Utc::now(),
                        event_type: EventType::SanctionsHit,
                        decision: Decision::Block,
                        transaction_context: context.clone(),
                        matched_address: address.to_string(),
                        address_type: address_type.clone(),
                        list_source: metadata.name.clone(),
                        entity_name: None,
                    });

                    return Ok(ScreeningResult {
                        decision: ScreeningDecision::Block {
                            reason: format!(
                                "Address is on {} sanctions list ({})",
                                metadata.name, address_type
                            ),
                        },
                        payer_address: payer.to_string(),
                        payee_address: payee.to_string(),
                        matched_entities,
                        list_versions,
                    });
                }
            }
        }

        // If we get here, transaction is clear
        self.audit_logger.log_event(ComplianceEvent {
            timestamp: chrono::Utc::now(),
            event_type: EventType::CleanTransaction,
            decision: Decision::Clear,
            transaction_context: context.clone(),
            matched_address: String::new(),
            address_type: AddressType::Payer,
            list_source: String::new(),
            entity_name: None,
        });

        Ok(ScreeningResult {
            decision: ScreeningDecision::Clear,
            payer_address: payer.to_string(),
            payee_address: payee.to_string(),
            matched_entities,
            list_versions,
        })
    }

    async fn screen_address(&self, address: &str) -> Result<ScreeningDecision> {
        // Check blacklist
        if let Some(blacklist) = &self.blacklist {
            if blacklist.is_blacklisted(address) {
                return Ok(ScreeningDecision::Block {
                    reason: "Address is blacklisted".to_string(),
                });
            }
        }

        // Check sanctions lists
        for list in &self.lists {
            if list.is_sanctioned(address) {
                let metadata = list.metadata();
                return Ok(ScreeningDecision::Block {
                    reason: format!("Address is on {} sanctions list", metadata.name),
                });
            }
        }

        Ok(ScreeningDecision::Clear)
    }

    fn is_list_enabled(&self, list_name: &str) -> bool {
        self.lists.iter().any(|list| list.metadata().name == list_name)
    }

    fn list_metadata(&self) -> HashMap<String, ListMetadata> {
        self.lists
            .iter()
            .map(|list| {
                let metadata = list.metadata();
                (metadata.name.clone(), metadata)
            })
            .collect()
    }

    async fn reload_lists(&mut self) -> Result<()> {
        // TODO: Implement list reloading in Phase 2
        Ok(())
    }
}

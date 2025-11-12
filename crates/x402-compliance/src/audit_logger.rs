use crate::checker::{AddressType, TransactionContext};
use crate::config::{AuditLoggingConfig, LogFormat};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ComplianceEvent {
    pub timestamp: DateTime<Utc>,
    pub event_type: EventType,
    pub decision: Decision,
    pub transaction_context: TransactionContext,
    pub matched_address: String,
    pub address_type: AddressType,
    pub list_source: String,
    pub entity_name: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EventType {
    SanctionsHit,
    BlacklistHit,
    CleanTransaction,
    ScreeningError,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Decision {
    Block,
    Review,
    Clear,
}

pub struct AuditLogger {
    config: AuditLoggingConfig,
}

impl AuditLogger {
    pub fn new(config: AuditLoggingConfig) -> Self {
        Self { config }
    }

    pub fn log_event(&self, event: ComplianceEvent) {
        if !self.config.enabled {
            return;
        }

        // Skip clean transactions if not configured to log them
        if matches!(event.decision, Decision::Clear) && !self.config.include_clear_transactions {
            return;
        }

        match self.config.format {
            LogFormat::Json => self.log_json(event),
            LogFormat::Text => self.log_text(event),
        }
    }

    fn log_json(&self, event: ComplianceEvent) {
        let json = serde_json::to_string(&event)
            .unwrap_or_else(|e| format!(r#"{{"error": "Failed to serialize: {}"}}"#, e));

        // Note: tracing macros require compile-time constant targets
        // Using the configured target as part of the log message instead
        match event.decision {
            Decision::Block => {
                tracing::error!(target: "compliance_audit", "{}", json)
            }
            Decision::Review => {
                tracing::warn!(target: "compliance_audit", "{}", json)
            }
            Decision::Clear => {
                tracing::info!(target: "compliance_audit", "{}", json)
            }
        }
    }

    fn log_text(&self, event: ComplianceEvent) {
        let message = format!(
            "[{}] {:?} - {} address: {} | List: {} | Network: {} | Amount: {} {}",
            event.timestamp.format("%Y-%m-%d %H:%M:%S"),
            event.decision,
            event.address_type,
            event.matched_address,
            event.list_source,
            event.transaction_context.network,
            event.transaction_context.amount,
            event.transaction_context.currency
        );

        // Note: tracing macros require compile-time constant targets
        match event.decision {
            Decision::Block => {
                tracing::error!(target: "compliance_audit", "{}", message)
            }
            Decision::Review => {
                tracing::warn!(target: "compliance_audit", "{}", message)
            }
            Decision::Clear => {
                tracing::info!(target: "compliance_audit", "{}", message)
            }
        }
    }
}

impl Default for AuditLogger {
    fn default() -> Self {
        Self::new(AuditLoggingConfig {
            enabled: true,
            target: "compliance_audit".to_string(),
            format: LogFormat::Json,
            include_clear_transactions: false,
        })
    }
}

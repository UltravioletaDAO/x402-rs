use crate::error::{ComplianceError, Result};
use base64::{engine::general_purpose, Engine as _};

/// Extractor for Solana addresses from transaction data
pub struct SolanaExtractor;

impl SolanaExtractor {
    /// Extract payer and payee addresses from a base64-encoded Solana transaction
    ///
    /// # Arguments
    /// * `transaction_base64` - Base64-encoded serialized Solana transaction
    ///
    /// # Returns
    /// * `Ok((payer, payee))` - Tuple of Solana public key strings
    /// * `Err(ComplianceError)` - If transaction cannot be parsed
    ///
    /// # Example
    /// ```ignore
    /// let (payer, payee) = SolanaExtractor::extract_addresses(&solana_payload.transaction)?;
    /// ```
    pub fn extract_addresses(transaction_base64: &str) -> Result<(String, String)> {
        // Decode base64
        let tx_bytes = general_purpose::STANDARD
            .decode(transaction_base64)
            .map_err(|e| {
                ComplianceError::AddressExtraction(format!("Failed to decode base64: {}", e))
            })?;

        // Deserialize transaction
        let transaction: solana_sdk::transaction::Transaction =
            bincode::deserialize(&tx_bytes).map_err(|e| {
                ComplianceError::AddressExtraction(format!(
                    "Failed to deserialize Solana transaction: {}",
                    e
                ))
            })?;

        // Extract payer (fee payer, first account key)
        let payer = transaction
            .message
            .account_keys
            .get(0)
            .ok_or_else(|| {
                ComplianceError::AddressExtraction("No payer account found".to_string())
            })?
            .to_string();

        // Extract payee (recipient, typically second account key for transfers)
        // Note: This is a simplified extraction. In reality, the recipient location
        // depends on the specific program instruction (SPL Token vs native SOL transfer)
        let payee = transaction
            .message
            .account_keys
            .get(1)
            .ok_or_else(|| {
                ComplianceError::AddressExtraction("No recipient account found".to_string())
            })?
            .to_string();

        Ok((payer, payee))
    }

    /// Extract all account keys from a transaction for comprehensive screening
    pub fn extract_all_accounts(transaction_base64: &str) -> Result<Vec<String>> {
        let tx_bytes = general_purpose::STANDARD
            .decode(transaction_base64)
            .map_err(|e| {
                ComplianceError::AddressExtraction(format!("Failed to decode base64: {}", e))
            })?;

        let transaction: solana_sdk::transaction::Transaction =
            bincode::deserialize(&tx_bytes).map_err(|e| {
                ComplianceError::AddressExtraction(format!(
                    "Failed to deserialize Solana transaction: {}",
                    e
                ))
            })?;

        let accounts: Vec<String> = transaction
            .message
            .account_keys
            .iter()
            .map(|key| key.to_string())
            .collect();

        Ok(accounts)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use solana_sdk::{
        message::Message,
        pubkey::Pubkey,
        signature::Keypair,
        signer::Signer,
        system_instruction,
        transaction::Transaction,
    };

    fn create_test_transaction() -> String {
        let payer = Keypair::new();
        let recipient = Keypair::new();

        // Create a simple SOL transfer instruction
        let instruction = system_instruction::transfer(
            &payer.pubkey(),
            &recipient.pubkey(),
            1_000_000, // 1 SOL (in lamports)
        );

        let message = Message::new(&[instruction], Some(&payer.pubkey()));
        let transaction = Transaction::new_unsigned(message);

        // Serialize and encode to base64
        let tx_bytes = bincode::serialize(&transaction).unwrap();
        general_purpose::STANDARD.encode(&tx_bytes)
    }

    #[test]
    fn test_extract_addresses() {
        let tx_base64 = create_test_transaction();
        let result = SolanaExtractor::extract_addresses(&tx_base64);

        assert!(result.is_ok());
        let (payer, payee) = result.unwrap();

        // Solana addresses should be base58-encoded (32-44 characters)
        assert!(!payer.is_empty());
        assert!(!payee.is_empty());
        assert_ne!(payer, payee);
    }

    #[test]
    fn test_extract_all_accounts() {
        let tx_base64 = create_test_transaction();
        let result = SolanaExtractor::extract_all_accounts(&tx_base64);

        assert!(result.is_ok());
        let accounts = result.unwrap();

        // Should have at least 2 accounts (payer + recipient)
        assert!(accounts.len() >= 2);
    }

    #[test]
    fn test_invalid_base64() {
        let result = SolanaExtractor::extract_addresses("invalid-base64!!!");
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_transaction_data() {
        let invalid_data = general_purpose::STANDARD.encode(b"not a transaction");
        let result = SolanaExtractor::extract_addresses(&invalid_data);
        assert!(result.is_err());
    }
}

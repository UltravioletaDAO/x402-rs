use crate::error::{ComplianceError, Result};

/// Extractor for EVM addresses from EIP-3009 authorization payloads
pub struct EvmExtractor;

impl EvmExtractor {
    /// Extract payer and payee addresses from any EVM authorization structure
    ///
    /// This method accepts any type that can provide from/to addresses via Display formatting.
    /// The addresses should format as "0x..." hex strings.
    ///
    /// # Arguments
    /// * `from_address` - The payer address (from field)
    /// * `to_address` - The payee address (to field)
    ///
    /// # Returns
    /// * `Ok((payer, payee))` - Tuple of normalized address strings
    /// * `Err(ComplianceError)` - If addresses cannot be extracted
    ///
    /// # Example
    /// ```ignore
    /// let (payer, payee) = EvmExtractor::extract_addresses(
    ///     &authorization.from,
    ///     &authorization.to
    /// )?;
    /// ```
    pub fn extract_addresses<T: std::fmt::Display>(
        from_address: &T,
        to_address: &T,
    ) -> Result<(String, String)> {
        let payer = format!("{}", from_address);
        let payee = format!("{}", to_address);

        // Basic validation: check that addresses look like Ethereum addresses
        if !Self::is_valid_eth_address(&payer) {
            return Err(ComplianceError::AddressExtraction(format!(
                "Invalid payer address format: {}",
                payer
            )));
        }

        if !Self::is_valid_eth_address(&payee) {
            return Err(ComplianceError::AddressExtraction(format!(
                "Invalid payee address format: {}",
                payee
            )));
        }

        Ok((payer, payee))
    }

    /// Validate that a string looks like an Ethereum address
    fn is_valid_eth_address(address: &str) -> bool {
        // Must start with 0x and be 42 characters total (0x + 40 hex digits)
        address.starts_with("0x") && address.len() == 42
    }

    /// Extract single address from Display type
    pub fn extract_single_address<T: std::fmt::Display>(address: &T) -> Result<String> {
        let addr = format!("{}", address);

        if !Self::is_valid_eth_address(&addr) {
            return Err(ComplianceError::AddressExtraction(format!(
                "Invalid address format: {}",
                addr
            )));
        }

        Ok(addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct MockAddress(String);

    impl std::fmt::Display for MockAddress {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.0)
        }
    }

    #[test]
    fn test_extract_valid_addresses() {
        let from = MockAddress("0x1234567890123456789012345678901234567890".to_string());
        let to = MockAddress("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd".to_string());

        let result = EvmExtractor::extract_addresses(&from, &to);
        assert!(result.is_ok());

        let (payer, payee) = result.unwrap();
        assert_eq!(payer, "0x1234567890123456789012345678901234567890");
        assert_eq!(payee, "0xabcdefabcdefabcdefabcdefabcdefabcdefabcd");
    }

    #[test]
    fn test_invalid_address_format() {
        let from = MockAddress("invalid".to_string());
        let to = MockAddress("0xabcdefabcdefabcdefabcdefabcdefabcdefabcd".to_string());

        let result = EvmExtractor::extract_addresses(&from, &to);
        assert!(result.is_err());
    }

    #[test]
    fn test_extract_single_address() {
        let addr = MockAddress("0x1234567890123456789012345678901234567890".to_string());
        let result = EvmExtractor::extract_single_address(&addr);
        assert!(result.is_ok());
        assert_eq!(
            result.unwrap(),
            "0x1234567890123456789012345678901234567890"
        );
    }
}

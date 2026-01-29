//! ERC-8004 Contract ABIs
//!
//! Solidity interface definitions for the ERC-8004 Trustless Agents registries.
//! Based on the official ERC-8004 specification: https://eips.ethereum.org/EIPS/eip-8004
//!
//! These ABIs match the canonical contracts deployed on:
//! - Ethereum Mainnet
//! - Ethereum Sepolia
//! - Base Mainnet (when deployed)
//! - Base Sepolia (when deployed)

use alloy::sol;

// ============================================================================
// Identity Registry ABI (Official ERC-8004 Spec)
// ============================================================================

sol!(
    #[sol(rpc)]
    /// ERC-8004 Identity Registry Interface
    ///
    /// The Identity Registry is an ERC-721 contract that provides censorship-resistant
    /// agent identifiers. Each agent gets a unique tokenId (agentId) and can set
    /// metadata including a URI pointing to their registration file.
    interface IIdentityRegistry {
        // ============ Events ============

        /// Emitted when a new agent is registered
        event Registered(
            uint256 indexed agentId,
            string agentURI,
            address indexed owner
        );

        /// Emitted when agent URI is updated
        event URIUpdated(
            uint256 indexed agentId,
            string newURI,
            address indexed updatedBy
        );

        /// Emitted when metadata is set
        event MetadataSet(
            uint256 indexed agentId,
            string indexed indexedMetadataKey,
            string metadataKey,
            bytes metadataValue
        );

        // ============ Registration Functions ============

        /// Register a new agent with URI and optional metadata
        /// @param agentURI URI pointing to the agent registration file
        /// @param metadata Array of key-value metadata entries
        /// @return agentId The newly assigned agent ID (ERC-721 tokenId)
        function register(
            string calldata agentURI,
            MetadataEntry[] calldata metadata
        ) external returns (uint256 agentId);

        /// Register a new agent with just a URI
        function register(string calldata agentURI) external returns (uint256 agentId);

        /// Register a new agent without any initial data
        function register() external returns (uint256 agentId);

        // ============ URI Management ============

        /// Update the agent's registration file URI
        /// @param agentId The agent's token ID
        /// @param newURI The new URI
        function setAgentURI(uint256 agentId, string calldata newURI) external;

        /// Get the agent's registration file URI (ERC-721 tokenURI)
        function tokenURI(uint256 agentId) external view returns (string memory);

        // ============ Wallet Management ============

        /// Set the agent's payment wallet address
        /// Requires EIP-712 signature from the wallet owner
        /// @param agentId The agent's token ID
        /// @param newWallet The wallet address to set
        /// @param deadline Signature expiration timestamp
        /// @param signature EIP-712 or ERC-1271 signature
        function setAgentWallet(
            uint256 agentId,
            address newWallet,
            uint256 deadline,
            bytes calldata signature
        ) external;

        /// Get the agent's payment wallet address
        function getAgentWallet(uint256 agentId) external view returns (address);

        /// Remove the agent's payment wallet
        function unsetAgentWallet(uint256 agentId) external;

        // ============ Metadata Management ============

        /// Get metadata value for a key
        function getMetadata(
            uint256 agentId,
            string calldata metadataKey
        ) external view returns (bytes memory);

        /// Set metadata value for a key
        function setMetadata(
            uint256 agentId,
            string calldata metadataKey,
            bytes calldata metadataValue
        ) external;

        // ============ ERC-721 Standard Functions ============

        /// Get the owner of an agent
        function ownerOf(uint256 agentId) external view returns (address);

        /// Get the total number of registered agents
        function totalSupply() external view returns (uint256);

        /// Check if an agent ID exists
        function exists(uint256 agentId) external view returns (bool);
    }

    /// Metadata entry for registration
    struct MetadataEntry {
        string metadataKey;
        bytes metadataValue;
    }
);

// ============================================================================
// Reputation Registry ABI (Official ERC-8004 Spec)
// ============================================================================

sol!(
    #[sol(rpc)]
    /// ERC-8004 Reputation Registry Interface
    ///
    /// The Reputation Registry provides a standardized interface for posting
    /// and fetching feedback signals. Anyone can submit feedback for any agent.
    interface IReputationRegistry {
        // ============ Events ============

        /// Emitted when new feedback is submitted
        event NewFeedback(
            uint256 indexed agentId,
            address indexed clientAddress,
            uint64 feedbackIndex,
            int128 value,
            uint8 valueDecimals,
            string indexed indexedTag1,
            string tag1,
            string tag2,
            string endpoint,
            string feedbackURI,
            bytes32 feedbackHash
        );

        /// Emitted when feedback is revoked
        event FeedbackRevoked(
            uint256 indexed agentId,
            address indexed clientAddress,
            uint64 indexed feedbackIndex
        );

        /// Emitted when a response is appended to feedback
        event ResponseAppended(
            uint256 indexed agentId,
            address indexed clientAddress,
            uint64 feedbackIndex,
            address indexed responder,
            string responseURI,
            bytes32 responseHash
        );

        // ============ Feedback Submission ============

        /// Submit feedback for an agent
        /// @param agentId The agent's token ID in the Identity Registry
        /// @param value Fixed-point feedback value
        /// @param valueDecimals Decimal places for value (0-18)
        /// @param tag1 Primary categorization tag
        /// @param tag2 Secondary categorization tag
        /// @param endpoint Optional service endpoint that was used
        /// @param feedbackURI Optional URI to off-chain feedback file
        /// @param feedbackHash Optional keccak256 hash of feedback content
        function giveFeedback(
            uint256 agentId,
            int128 value,
            uint8 valueDecimals,
            string calldata tag1,
            string calldata tag2,
            string calldata endpoint,
            string calldata feedbackURI,
            bytes32 feedbackHash
        ) external;

        /// Revoke previously submitted feedback
        /// @param agentId The agent's token ID
        /// @param feedbackIndex The index of the feedback to revoke (1-indexed)
        function revokeFeedback(uint256 agentId, uint64 feedbackIndex) external;

        /// Append a response to feedback (typically by the agent)
        /// @param agentId The agent's token ID
        /// @param clientAddress The original feedback submitter
        /// @param feedbackIndex The feedback index to respond to
        /// @param responseURI URI to the response content
        /// @param responseHash Hash of the response content
        function appendResponse(
            uint256 agentId,
            address clientAddress,
            uint64 feedbackIndex,
            string calldata responseURI,
            bytes32 responseHash
        ) external;

        // ============ Read Functions ============

        /// Get the Identity Registry address
        function getIdentityRegistry() external view returns (address);

        /// Get aggregated reputation summary
        /// @param agentId The agent's token ID
        /// @param clientAddresses Filter by specific clients (empty = all)
        /// @param tag1 Filter by tag1 (empty = all)
        /// @param tag2 Filter by tag2 (empty = all)
        /// @return count Number of matching feedback entries
        /// @return summaryValue Aggregated value
        /// @return summaryValueDecimals Decimal places for summaryValue
        function getSummary(
            uint256 agentId,
            address[] calldata clientAddresses,
            string calldata tag1,
            string calldata tag2
        ) external view returns (
            uint64 count,
            int128 summaryValue,
            uint8 summaryValueDecimals
        );

        /// Read a specific feedback entry
        /// @param agentId The agent's token ID
        /// @param clientAddress The feedback submitter
        /// @param feedbackIndex The feedback index (1-indexed)
        function readFeedback(
            uint256 agentId,
            address clientAddress,
            uint64 feedbackIndex
        ) external view returns (
            int128 value,
            uint8 valueDecimals,
            string memory tag1,
            string memory tag2,
            bool isRevoked
        );

        /// Read all feedback with filters
        function readAllFeedback(
            uint256 agentId,
            address[] calldata clientAddresses,
            string calldata tag1,
            string calldata tag2,
            bool includeRevoked
        ) external view returns (
            address[] memory clients,
            uint64[] memory feedbackIndexes,
            int128[] memory values,
            uint8[] memory valueDecimals,
            string[] memory tag1s,
            string[] memory tag2s,
            bool[] memory revokedStatuses
        );

        /// Get response count for a feedback entry
        function getResponseCount(
            uint256 agentId,
            address clientAddress,
            uint64 feedbackIndex,
            address[] calldata responders
        ) external view returns (uint64 count);

        /// Get all clients who have given feedback to an agent
        function getClients(uint256 agentId) external view returns (address[] memory);

        /// Get the last feedback index for a client-agent pair
        function getLastIndex(
            uint256 agentId,
            address clientAddress
        ) external view returns (uint64);
    }
);

// ============================================================================
// Validation Registry ABI (Official ERC-8004 Spec)
// ============================================================================

sol!(
    #[sol(rpc)]
    /// ERC-8004 Validation Registry Interface
    ///
    /// The Validation Registry provides hooks for independent validators
    /// (stakers, zkML verifiers, TEEs, judges) to publish validation results.
    interface IValidationRegistry {
        // ============ Events ============

        /// Emitted when a validation is requested
        event ValidationRequest(
            address indexed validatorAddress,
            uint256 indexed agentId,
            string requestURI,
            bytes32 indexed requestHash
        );

        /// Emitted when a validation response is submitted
        event ValidationResponse(
            address indexed validatorAddress,
            uint256 indexed agentId,
            bytes32 indexed requestHash,
            uint8 response,
            string responseURI,
            bytes32 responseHash,
            string tag
        );

        // ============ Validation Functions ============

        /// Request validation from a specific validator
        /// @param validatorAddress The validator to request
        /// @param agentId The agent to validate
        /// @param requestURI URI to validation request details
        /// @param requestHash Hash of the request content
        function validationRequest(
            address validatorAddress,
            uint256 agentId,
            string calldata requestURI,
            bytes32 requestHash
        ) external;

        /// Submit a validation response
        /// @param requestHash The request being responded to
        /// @param response Validation result (0-100, 0=failed, 100=passed)
        /// @param responseURI URI to response details
        /// @param responseHash Hash of response content
        /// @param tag Categorization tag (e.g., "soft-finality", "hard-finality")
        function validationResponse(
            bytes32 requestHash,
            uint8 response,
            string calldata responseURI,
            bytes32 responseHash,
            string calldata tag
        ) external;

        // ============ Read Functions ============

        /// Get the Identity Registry address
        function getIdentityRegistry() external view returns (address);

        /// Get validation status for a request
        function getValidationStatus(bytes32 requestHash) external view returns (
            address validatorAddress,
            uint256 agentId,
            uint8 response,
            bytes32 responseHash,
            string memory tag,
            uint256 lastUpdate
        );

        /// Get validation summary for an agent
        function getSummary(
            uint256 agentId,
            address[] calldata validatorAddresses,
            string calldata tag
        ) external view returns (
            uint64 count,
            uint8 averageResponse
        );

        /// Get all validations for an agent
        function getAgentValidations(uint256 agentId) external view returns (bytes32[] memory requestHashes);

        /// Get all requests assigned to a validator
        function getValidatorRequests(address validatorAddress) external view returns (bytes32[] memory requestHashes);
    }
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identity_registry_abi() {
        // Test that the ABI compiles and call structs work
        let _ = IIdentityRegistry::ownerOfCall {
            agentId: alloy::primitives::U256::ZERO,
        };

        let _ = IIdentityRegistry::getAgentWalletCall {
            agentId: alloy::primitives::U256::ZERO,
        };

        let _ = IIdentityRegistry::getMetadataCall {
            agentId: alloy::primitives::U256::ZERO,
            metadataKey: String::new(),
        };
    }

    #[test]
    fn test_reputation_registry_abi() {
        let _ = IReputationRegistry::getSummaryCall {
            agentId: alloy::primitives::U256::ZERO,
            clientAddresses: vec![],
            tag1: String::new(),
            tag2: String::new(),
        };

        let _ = IReputationRegistry::getClientsCall {
            agentId: alloy::primitives::U256::ZERO,
        };

        let _ = IReputationRegistry::readFeedbackCall {
            agentId: alloy::primitives::U256::ZERO,
            clientAddress: alloy::primitives::Address::ZERO,
            feedbackIndex: 0,
        };
    }

    #[test]
    fn test_validation_registry_abi() {
        let _ = IValidationRegistry::getValidationStatusCall {
            requestHash: alloy::primitives::FixedBytes::ZERO,
        };

        let _ = IValidationRegistry::getAgentValidationsCall {
            agentId: alloy::primitives::U256::ZERO,
        };
    }
}

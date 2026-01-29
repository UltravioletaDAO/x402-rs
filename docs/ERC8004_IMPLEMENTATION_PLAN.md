# ERC-8004 Implementation Plan for x402-rs Facilitator

## Executive Summary

After comprehensive analysis of the ERC-8004 specification, official documentation, SDKs, and community resources, this document outlines what we have implemented vs. what the full spec provides, and recommends next steps.

**Research Sources:**
- [EIP-8004 Official Specification](https://eips.ethereum.org/EIPS/eip-8004)
- [8004.org Official Website](https://8004.org/)
- [Ethereum Magicians Discussion](https://ethereum-magicians.org/t/erc-8004-trustless-agents/25098)
- [Official Contracts Repository](https://github.com/erc-8004/erc-8004-contracts)
- [JavaScript SDK](https://github.com/tetratorus/erc-8004-js)
- [Python SDK](https://github.com/tetratorus/erc-8004-py)
- [Awesome ERC-8004 Resources](https://github.com/sudeepb02/awesome-erc8004)

---

## Contract Addresses (Discovered)

### Ethereum Mainnet (PRODUCTION)
| Contract | Address |
|----------|---------|
| IdentityRegistry | `0x8004A169FB4a3325136EB29fA0ceB6D2e539a432` |
| ReputationRegistry | `0x8004BAa17C55a88189AE136b182e5fdA19dE9b63` |
| ValidationRegistry | Not deployed yet |

### Ethereum Sepolia (TESTNET)
| Contract | Address |
|----------|---------|
| IdentityRegistry | `0x8004A818BFB912233c491871b3d84c89A494BD9e` |
| ReputationRegistry | `0x8004B663056A597Dffe9eCcC1965A193B7388713` |
| ValidationRegistry | `0x8004Cb1BF31DAf7788923b405b754f57acEB4272` |

---

## Current Implementation Status

### What We Have (v1.22.4)

| Feature | Status | Notes |
|---------|--------|-------|
| POST /feedback | BASIC | Only calls `giveFeedback()` with minimal params |
| GET /feedback | DONE | Returns schema and supported networks |
| ProofOfPayment | DONE | Generated during settlement with `8004-reputation` extension |
| ReputationRegistry address | CONFIGURABLE | Via AWS Secrets Manager |
| IdentityRegistry address | CONFIGURABLE | Via AWS Secrets Manager |
| Ethereum Mainnet | SUPPORTED | Only network currently enabled |
| Landing page docs | DONE | Added to API endpoints section |

### What's Missing from Full Spec

The ERC-8004 specification defines THREE registries with comprehensive functionality. Our implementation only touches ~10% of the Reputation Registry.

---

## ERC-8004 Component Analysis

### 1. Identity Registry (NOT IMPLEMENTED)

The Identity Registry is an ERC-721 contract that provides censorship-resistant agent identifiers.

**Functions Available:**
```solidity
// Registration
function register(string agentURI, MetadataEntry[] calldata metadata) returns (uint256 agentId)
function register(string agentURI) returns (uint256 agentId)
function register() returns (uint256 agentId)

// URI Management
function setAgentURI(uint256 agentId, string calldata newURI)
function tokenURI(uint256 agentId) returns (string)

// Wallet Management (payment address)
function setAgentWallet(uint256 agentId, address newWallet, uint256 deadline, bytes signature)
function getAgentWallet(uint256 agentId) returns (address)
function unsetAgentWallet(uint256 agentId)

// Metadata
function getMetadata(uint256 agentId, string metadataKey) returns (bytes)
function setMetadata(uint256 agentId, string metadataKey, bytes metadataValue)
```

**Events:**
- `Registered(agentId, agentURI, owner)`
- `URIUpdated(agentId, newURI, updatedBy)`
- `MetadataSet(agentId, metadataKey, metadataValue)`

**USE CASES for Facilitator:**
- Register the facilitator itself as an ERC-8004 agent
- Allow agents to register through the facilitator
- Resolve agent metadata before accepting payments
- Verify payee is a registered agent

### 2. Reputation Registry (PARTIALLY IMPLEMENTED)

**What We Implemented:**
- `giveFeedback()` - Basic version without all parameters

**Full Function Signature (not fully implemented):**
```solidity
function giveFeedback(
    uint256 agentId,
    int128 value,
    uint8 valueDecimals,
    string calldata tag1,       // NOT USED
    string calldata tag2,       // NOT USED
    string calldata endpoint,   // NOT USED
    string calldata feedbackURI,// NOT USED
    bytes32 feedbackHash        // NOT USED
) external
```

**Additional Functions NOT Implemented:**
```solidity
// Feedback Management
function revokeFeedback(uint256 agentId, uint64 feedbackIndex)
function appendResponse(uint256 agentId, address clientAddress, uint64 feedbackIndex,
                        string responseURI, bytes32 responseHash)

// Reading Feedback
function getSummary(uint256 agentId, address[] clientAddresses, string tag1, string tag2)
    returns (uint64 count, int128 summaryValue, uint8 summaryValueDecimals)
function readFeedback(uint256 agentId, address clientAddress, uint64 feedbackIndex)
    returns (int128 value, uint8 valueDecimals, string tag1, string tag2, bool isRevoked)
function readAllFeedback(uint256 agentId, address[] clientAddresses, string tag1, string tag2, bool includeRevoked)
    returns (...)
function getClients(uint256 agentId) returns (address[])
function getLastIndex(uint256 agentId, address clientAddress) returns (uint64)
```

### 3. Validation Registry (NOT IMPLEMENTED)

The Validation Registry provides hooks for independent validators (stakers, zkML verifiers, TEEs).

**Functions:**
```solidity
function validationRequest(address validatorAddress, uint256 agentId,
                           string requestURI, bytes32 requestHash)
function validationResponse(bytes32 requestHash, uint8 response,
                            string responseURI, bytes32 responseHash, string tag)
function getValidationStatus(bytes32 requestHash)
    returns (address validatorAddress, uint256 agentId, uint8 response,
             bytes32 responseHash, string tag, uint256 lastUpdate)
function getSummary(uint256 agentId, address[] validatorAddresses, string tag)
    returns (uint64 count, uint8 averageResponse)
function getAgentValidations(uint256 agentId) returns (bytes32[])
function getValidatorRequests(address validatorAddress) returns (bytes32[])
```

---

## Detailed Implementation Plan

### Phase 1: Complete Reputation Registry Integration (PRIORITY: HIGH)

**Task 1.1: Enhance giveFeedback with full parameters**
- Add support for `tag1`, `tag2` (categorization)
- Add support for `endpoint` (service endpoint that was used)
- Add support for `feedbackURI` (IPFS link to detailed feedback)
- Add support for `feedbackHash` (keccak256 for integrity)

**Task 1.2: Add Reputation Read Endpoints**
```
GET /reputation/:agentId              -> getSummary()
GET /reputation/:agentId/feedback     -> readAllFeedback()
GET /reputation/:agentId/clients      -> getClients()
```

**Task 1.3: Add Feedback Management Endpoints**
```
DELETE /feedback/:agentId/:index      -> revokeFeedback()
POST /feedback/:agentId/:index/response -> appendResponse()
```

**Task 1.4: Add Sepolia Testnet Support**
- Configure Sepolia contract addresses
- Enable `ethereum-sepolia` network for ERC-8004

### Phase 2: Identity Registry Integration (PRIORITY: MEDIUM)

**Task 2.1: Add Identity Endpoints**
```
POST /identity/register              -> register()
GET /identity/:agentId               -> tokenURI() + metadata
PUT /identity/:agentId/uri           -> setAgentURI()
PUT /identity/:agentId/wallet        -> setAgentWallet()
GET /identity/:agentId/wallet        -> getAgentWallet()
```

**Task 2.2: Agent Resolution in Payments**
- Before settlement, optionally verify payee is registered agent
- Resolve agent metadata to get preferred payment address
- Add `verifyAgent` option to PaymentRequirements

**Task 2.3: Register Facilitator as Agent**
- Create registration file for the facilitator
- Register on Ethereum Mainnet
- Publish agentURI

### Phase 3: Validation Registry Integration (PRIORITY: LOW)

**Task 3.1: Add Validation Endpoints**
```
POST /validation/request             -> validationRequest()
POST /validation/:hash/response      -> validationResponse()
GET /validation/:hash                -> getValidationStatus()
GET /validation/agent/:agentId       -> getAgentValidations()
```

**Task 3.2: Integrate with Settlement**
- Optionally require validation before large settlements
- Check agent validation status before processing payments

### Phase 4: Enhanced Features (PRIORITY: LOW)

**Task 4.1: IPFS Integration**
- Support for uploading feedback to IPFS
- Support for resolving agent registration files from IPFS
- Pinning service integration (Pinata, web3.storage)

**Task 4.2: Off-chain Feedback File Support**
```json
{
  "type": "https://eips.ethereum.org/EIPS/eip-8004#feedback-v1",
  "feedback": [{
    "timestamp": 1234567890,
    "result": "success",
    "value": 100,
    "valueDecimals": 0,
    "proofOfPayment": {
      "fromAddress": "0x...",
      "toAddress": "0x...",
      "chainId": "1",
      "txHash": "0x..."
    }
  }]
}
```

**Task 4.3: Multi-chain Support**
When contracts are deployed on other networks:
- Base Sepolia
- Linea Sepolia
- Polygon Amoy
- HyperEVM (when available)

---

## Recommendations

### Immediate Actions (This Week)

1. **Add Sepolia testnet support** - Contracts are already deployed
   - Allows testing without spending mainnet gas
   - Update `is_erc8004_supported()` to include `Network::EthereumSepolia`
   - Add Sepolia contract addresses to AWS Secrets Manager

2. **Enhance feedback endpoint** - Add missing parameters
   - `tag1`, `tag2` for categorization
   - `endpoint` for service tracking
   - This matches what the SDKs expect

### Short-term Actions (Next 2 Weeks)

3. **Add reputation read endpoints** - Essential for clients
   - `GET /reputation/:agentId` - Get agent's reputation score
   - Users need to READ reputation, not just write it

4. **Add revokeFeedback support** - Users should be able to revoke
   - `DELETE /feedback/:agentId/:index`

### Medium-term Actions (Next Month)

5. **Identity Registry integration** - Agent discovery
   - Register facilitator as an ERC-8004 agent
   - Add `/identity` endpoints

6. **Documentation alignment** - Match SDK docs
   - Update SDK integration guide to use official SDK patterns
   - Add TypeScript/Python examples using official SDKs

### Optional/Future

7. **Validation Registry** - When use case is clear
8. **IPFS integration** - For detailed feedback storage
9. **Multi-chain expansion** - As contracts are deployed

---

## Current vs Target Feature Matrix

| Feature | Current | Target | Priority |
|---------|---------|--------|----------|
| giveFeedback (basic) | YES | YES | - |
| giveFeedback (full params) | NO | YES | HIGH |
| revokeFeedback | NO | YES | HIGH |
| appendResponse | NO | YES | MEDIUM |
| getSummary | NO | YES | HIGH |
| readFeedback | NO | YES | MEDIUM |
| readAllFeedback | NO | YES | LOW |
| Ethereum Mainnet | YES | YES | - |
| Ethereum Sepolia | NO | YES | HIGH |
| Identity: register | NO | YES | MEDIUM |
| Identity: getAgentWallet | NO | YES | MEDIUM |
| Identity: resolution | NO | YES | MEDIUM |
| Validation: request | NO | OPTIONAL | LOW |
| Validation: response | NO | OPTIONAL | LOW |
| IPFS integration | NO | OPTIONAL | LOW |

---

## Estimated Effort

| Phase | Tasks | Effort | Dependencies |
|-------|-------|--------|--------------|
| Phase 1 | Reputation complete | 3-5 days | None |
| Phase 2 | Identity integration | 3-4 days | Phase 1 |
| Phase 3 | Validation integration | 2-3 days | Phase 2 |
| Phase 4 | Enhanced features | 5-7 days | Phase 1-3 |

**Total for full spec compliance: ~2-3 weeks**
**MVP improvements (Phase 1): ~3-5 days**

---

## Questions for Decision

1. **Sepolia Testnet**: Should we enable it now for testing?
2. **Reputation Read Endpoints**: Should users be able to query reputation through the facilitator, or should they use direct RPC calls?
3. **Identity Integration**: Should we verify payees are registered agents before settlement?
4. **IPFS**: Do we need IPFS integration, or is on-chain only sufficient?
5. **Multi-chain**: Which chains should we prioritize as contracts get deployed?

---

## Resources

### Official SDKs
- **JavaScript**: `npm install erc-8004-js`
- **Python**: `pip install erc-8004-py`

### Contract ABIs
Available at: https://github.com/erc-8004/erc-8004-contracts/tree/main/abis

### Community
- Telegram: http://t.me/ERC8004
- Builder Program: Contact davide.crapis@ethereum.org

---

*Document created: January 29, 2026*
*Based on ERC-8004 specification and official resources*

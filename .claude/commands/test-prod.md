Test the production facilitator deployment at https://facilitator.ultravioletadao.xyz:

**Basic Health Checks:**
1. Check health endpoint: `curl https://facilitator.ultravioletadao.xyz/health`
   - Expected: `{"status":"healthy"}`

2. Verify branding is intact: `curl https://facilitator.ultravioletadao.xyz/ | grep -i "Ultravioleta"`
   - Expected: Should find "Ultravioleta" string in HTML

3. Check landing page size: `curl -s https://facilitator.ultravioletadao.xyz/ | wc -c`
   - Expected: ~57000 bytes (custom Ultravioleta DAO landing page)

**Network Configuration:**
4. List supported networks: `curl https://facilitator.ultravioletadao.xyz/supported | jq`
   - Expected: JSON with `kinds` array

5. Count networks: `curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds | length'`
   - Expected: **26 networks** (EVM: 18, Solana/Fogo: 4, NEAR: 2, Stellar: 2)

6. **CRITICAL**: Verify ALL network families are present:
   ```bash
   # Check for all 4 blockchain ecosystems
   curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network' | sort
   ```
   Must include:
   - EVM (18): base, base-sepolia, avalanche, avalanche-fuji, polygon, polygon-amoy, optimism, optimism-sepolia, celo, celo-sepolia, hyperevm, hyperevm-testnet, ethereum, ethereum-sepolia, arbitrum, arbitrum-sepolia, unichain, unichain-sepolia
   - Solana/SVM (4): solana, solana-devnet, fogo, fogo-testnet
   - NEAR (2): near, near-testnet
   - Stellar (2): stellar, stellar-testnet

7. Verify NEAR is working:
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | test("near"))'
   ```
   - Expected: near with feePayer `uvd-facilitator.near`, near-testnet with `uvd-facilitator.testnet`

8. Verify Stellar is working:
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[] | select(.network | test("stellar"))'
   ```
   - Expected: stellar and stellar-testnet with feePayer starting with `G...`

**Security & Blacklist:**
9. Check blacklist endpoint: `curl https://facilitator.ultravioletadao.xyz/blacklist | jq`
   - Expected:
     ```json
     {
       "totalBlocked": [number > 0],
       "evmCount": [number],
       "solanaCount": [number],
       "entries": [...],
       "source": "config/blacklist.json",
       "loadedAtStartup": true
     }
     ```

10. Verify critical malicious wallet is blocked:
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/blacklist | jq '.entries[] | select(.wallet | contains("41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az"))'
   ```
   - Expected: Should find the malicious Solana wallet

11. Verify blacklist loaded at startup:
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/blacklist | jq '.loadedAtStartup'
   ```
   - Expected: `true` (CRITICAL - if false, blacklist is not protecting the facilitator!)

**CloudWatch Logs Check (Optional):**
12. Get latest task ID and check logs for wallet separation:
    ```bash
    aws ecs list-tasks --cluster facilitator-production --service-name facilitator-production --desired-status RUNNING --region us-east-2
    MSYS_NO_PATHCONV=1 aws logs get-log-events --log-group-name /ecs/facilitator-production --log-stream-name "ecs/facilitator/[task-id]" --region us-east-2 --start-from-head --limit 30 --query 'events[*].message' --output text | grep -E "(testnet|mainnet)"
    ```
    - Expected: Should see separate wallet addresses for testnet (0x3403...) and mainnet (0x1030...)

**Test Results Summary:**
Report all test results with clear ✓ PASS / ✗ FAIL status for each check.

**Critical Failure Indicators:**
- ✗ Health check fails → Service is down
- ✗ Landing page < 50KB → Branding was lost (revert to upstream)
- ✗ Network count < 26 → **Missing networks - check secrets configuration**
- ✗ NEAR networks missing → **CRITICAL: NEAR secrets not configured in task definition**
- ✗ Stellar networks missing → **CRITICAL: Stellar secrets not configured in task definition**
- ✗ `loadedAtStartup: false` → **CRITICAL: Blacklist not working, facilitator vulnerable to draining**
- ✗ `totalBlocked: 0` → **CRITICAL: No addresses blocked, facilitator vulnerable**

**If networks are missing:**
1. Check logs for warnings: `aws logs tail /ecs/facilitator-production --since 5m --region us-east-2 | grep -i "skipping\|warn"`
2. Verify task definition secrets: `aws ecs describe-task-definition --task-definition facilitator-production --region us-east-2 | jq '.taskDefinition.containerDefinitions[0].secrets[].name' | sort`
3. Run validation: `cd terraform/environments/production && bash validate_secrets.sh us-east-2`

**Success Criteria:**
All checks must pass, especially:
- ✓ Health endpoint responding
- ✓ **26 networks present** (EVM + Solana + NEAR + Stellar)
- ✓ NEAR showing uvd-facilitator.near / uvd-facilitator.testnet
- ✓ Stellar showing public keys starting with G...
- ✓ Blacklist loaded at startup
- ✓ Total blocked > 0
- ✓ Malicious wallet 41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az is in blocklist

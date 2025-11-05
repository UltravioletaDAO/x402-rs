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
   - Expected: 14+ networks

6. Verify custom networks are present (HyperEVM, Polygon, Optimism, Celo, Solana):
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/supported | jq '.kinds[].network' | grep -E "(hyperevm|polygon|optimism|celo|solana)"
   ```
   - Expected: All 5 custom network families present

**Security & Blacklist:**
7. Check blacklist endpoint: `curl https://facilitator.ultravioletadao.xyz/blacklist | jq`
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

8. Verify critical malicious wallet is blocked:
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/blacklist | jq '.entries[] | select(.wallet | contains("41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az"))'
   ```
   - Expected: Should find the malicious Solana wallet

9. Verify blacklist loaded at startup:
   ```bash
   curl -s https://facilitator.ultravioletadao.xyz/blacklist | jq '.loadedAtStartup'
   ```
   - Expected: `true` (CRITICAL - if false, blacklist is not protecting the facilitator!)

**CloudWatch Logs Check (Optional):**
10. Get latest task ID and check logs for wallet separation:
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
- ✗ Custom networks missing → Network configuration broken
- ✗ `loadedAtStartup: false` → **CRITICAL: Blacklist not working, facilitator vulnerable to draining**
- ✗ `totalBlocked: 0` → **CRITICAL: No addresses blocked, facilitator vulnerable**

If any tests fail, provide specific details about what went wrong and suggest troubleshooting steps from the CLAUDE.md documentation.

**Success Criteria:**
All checks must pass, especially:
- ✓ Health endpoint responding
- ✓ Blacklist loaded at startup
- ✓ Total blocked > 0
- ✓ Malicious wallet 41fx2QjU8qCEPPDLWnypgxaHaDJ3dFVi8BhfUmTEQ3az is in blocklist

Test the production facilitator deployment at https://facilitator.ultravioletadao.xyz:

1. Check health endpoint: `curl https://facilitator.ultravioletadao.xyz/health`
2. Verify branding is intact: `curl https://facilitator.ultravioletadao.xyz/ | grep -i "Ultravioleta"`
3. List supported networks: `curl https://facilitator.ultravioletadao.xyz/supported | jq`
4. Verify custom networks are present (HyperEVM, Polygon, Optimism, Celo, Solana)
5. Check that the landing page loads correctly by fetching it and verifying it's approximately 57KB
6. Report all test results with clear pass/fail status

If any tests fail, provide specific details about what went wrong and suggest troubleshooting steps from the CLAUDE.md documentation.

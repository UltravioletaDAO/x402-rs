/**
 * Create a Crossmint smart wallet on Solana mainnet.
 *
 * Run once:  node setup-wallet.mjs
 * Requires CROSSMINT_API_KEY in .env (sk_production_...)
 */
import "dotenv/config";

const API_KEY = process.env.CROSSMINT_API_KEY;
if (!API_KEY) {
  console.error("ERROR: Set CROSSMINT_API_KEY in .env");
  process.exit(1);
}

const isProduction = API_KEY.startsWith("sk_production_");
const isStaging = API_KEY.startsWith("sk_staging_");
const baseUrl = isProduction
  ? "https://www.crossmint.com/api"
  : "https://staging.crossmint.com/api";

console.log("\n=== Create Crossmint Smart Wallet ===\n");
console.log(`Environment: ${isProduction ? "production (mainnet)" : "staging (devnet)"}`);
console.log(`API base:    ${baseUrl}\n`);

const response = await fetch(`${baseUrl}/2025-06-09/wallets`, {
  method: "POST",
  headers: {
    "X-API-KEY": API_KEY,
    "Content-Type": "application/json",
  },
  body: JSON.stringify({
    chainType: "solana",
    type: "smart",
    config: {
      adminSigner: { type: "api-key" },
    },
  }),
});

if (!response.ok) {
  const text = await response.text();
  console.error(`ERROR: Crossmint API returned ${response.status}`);
  console.error(text);
  process.exit(1);
}

const wallet = await response.json();

console.log("Wallet created!\n");
console.log(`  Address: ${wallet.address}`);
console.log(`  Type:    ${wallet.type}`);
console.log(`  Chain:   ${wallet.chainType || "solana"}`);

console.log(`\nAdd to your .env:`);
console.log(`  CROSSMINT_WALLET=${wallet.address}\n`);

if (isProduction) {
  console.log("Next steps:");
  console.log("  1. Send 0.01 SOL to the wallet for fees");
  console.log("  2. Send 0.02 USDC to the wallet for the test payment");
  console.log("  3. Start server:   node server.mjs");
  console.log("  4. Run test:       node test.mjs\n");
} else {
  console.log("Next steps:");
  console.log("  1. Fund with SOL:  https://faucet.solana.com");
  console.log("  2. Fund with USDC: https://faucet.circle.com (Solana devnet)");
  console.log("  3. Start server:   node server.mjs");
  console.log("  4. Run test:       node test.mjs\n");
}

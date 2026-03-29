/**
 * Test Crossmint smart wallet x402 payment on Solana mainnet
 * against the Ultravioleta DAO facilitator.
 *
 * Prerequisites:
 *   1. cp .env.example .env  (fill in CROSSMINT_API_KEY and CROSSMINT_WALLET)
 *   2. Fund wallet: ~0.01 SOL (fees) + 0.02 USDC
 *   3. Start paywall:  node server.mjs
 *   4. Run test:        node test.mjs
 *
 * RPC resolution: AWS Secrets Manager > RPC_URL env > public fallback
 */
import "dotenv/config";
import { execSync } from "child_process";
import { Connection, PublicKey } from "@solana/web3.js";
import { getAssociatedTokenAddress, getAccount } from "@solana/spl-token";
import { createCrossmintWallet } from "@faremeter/wallet-crossmint";
import { lookupKnownSPLToken } from "@faremeter/info/solana";
import { createPaymentHandler } from "@faremeter/payment-solana/exact";
import { wrap as wrapFetch } from "@faremeter/fetch";

/**
 * Resolve Solana RPC URL. Prefers AWS Secrets Manager over public endpoint.
 * Never logs the actual URL to avoid leaking API keys in streams/CI.
 */
function resolveRpcUrl() {
  const envUrl = process.env.RPC_URL;
  const PUBLIC = "https://api.mainnet-beta.solana.com";

  if (envUrl && envUrl !== PUBLIC) {
    console.log("[rpc] Using RPC_URL from environment (custom endpoint)");
    return envUrl;
  }

  try {
    const raw = execSync(
      'aws secretsmanager get-secret-value --secret-id facilitator-rpc-mainnet --region us-east-2 --query SecretString --output text',
      { encoding: "utf8", timeout: 10_000, stdio: ["pipe", "pipe", "pipe"] }
    ).trim();
    const secrets = JSON.parse(raw);
    if (secrets.solana) {
      console.log("[rpc] Using premium RPC from AWS Secrets Manager");
      return secrets.solana;
    }
  } catch {
    // AWS CLI not available or no credentials
  }

  console.log("[rpc] WARNING: Using public RPC (rate-limited). Set RPC_URL or configure AWS credentials.");
  return PUBLIC;
}

// ── Config ──────────────────────────────────────────────────────────

const CROSSMINT_API_KEY = process.env.CROSSMINT_API_KEY;
const CROSSMINT_WALLET = process.env.CROSSMINT_WALLET;
const RPC_URL = resolveRpcUrl();
const SERVER_URL = `http://localhost:${process.env.SERVER_PORT || 3402}`;

if (!CROSSMINT_API_KEY || !CROSSMINT_WALLET) {
  console.error("ERROR: Set CROSSMINT_API_KEY and CROSSMINT_WALLET in .env");
  process.exit(1);
}

// ── Setup ───────────────────────────────────────────────────────────

console.log("\n=== Crossmint Smart Wallet x402 Test (Solana Mainnet) ===\n");
console.log(`Wallet:      ${CROSSMINT_WALLET}`);
console.log(`RPC:         ${RPC_URL.includes("quiknode") || RPC_URL.includes("helius") ? "(premium - hidden)" : RPC_URL}`);
console.log(`Server:      ${SERVER_URL}`);

const connection = new Connection(RPC_URL, "confirmed");
const usdcInfo = lookupKnownSPLToken("mainnet-beta", "USDC");
const mint = new PublicKey(usdcInfo.address);

console.log(`USDC Mint:   ${usdcInfo.address}\n`);

// ── Step 1: Check server is running ─────────────────────────────────

console.log("[1/4] Checking paywall server...");
try {
  const health = await fetch(`${SERVER_URL}/health`);
  if (!health.ok) throw new Error(`Server returned ${health.status}`);
  console.log("      Server is running.\n");
} catch (e) {
  console.error(`      ERROR: Cannot reach ${SERVER_URL}/health`);
  console.error("      Start the server first: node server.mjs\n");
  process.exit(1);
}

// ── Step 2: Check wallet balances ───────────────────────────────────

console.log("[2/4] Checking wallet balances...");
const walletPubkey = new PublicKey(CROSSMINT_WALLET);

const solBalance = await connection.getBalance(walletPubkey);
console.log(`      SOL:  ${(solBalance / 1e9).toFixed(6)} SOL`);
if (solBalance < 2_000_000) {
  console.error("      ERROR: Need at least 0.002 SOL for fees.");
  process.exit(1);
}

try {
  // allowOwnerOffCurve=true is critical for smart wallet PDAs
  const ata = await getAssociatedTokenAddress(mint, walletPubkey, true);
  const tokenAccount = await getAccount(connection, ata);
  const usdcBalance = Number(tokenAccount.amount) / 1e6;
  console.log(`      USDC: ${usdcBalance.toFixed(4)} USDC`);
  if (usdcBalance < 0.01) {
    console.error("      ERROR: Need at least 0.01 USDC.");
    process.exit(1);
  }
} catch (e) {
  console.error("      ERROR: No USDC token account found for this wallet.");
  console.error("      Send at least 0.02 USDC to:", CROSSMINT_WALLET);
  process.exit(1);
}
console.log("");

// ── Step 3: Create Crossmint wallet + payment handler ───────────────

console.log("[3/4] Initializing Crossmint wallet...");
const wallet = await createCrossmintWallet(
  "mainnet-beta",
  CROSSMINT_API_KEY,
  CROSSMINT_WALLET
);
console.log("      Crossmint wallet connected.\n");

const paymentHandler = createPaymentHandler(wallet, mint, connection, {
  features: { enableSettlementAccounts: true },
  token: { allowOwnerOffCurve: true }, // required for smart wallet PDAs
});

const fetchWithPayer = wrapFetch(fetch, { handlers: [paymentHandler] });

// ── Step 4: Make x402 payment ───────────────────────────────────────

console.log("[4/4] Making x402 payment (0.01 USDC via Crossmint smart wallet)...");
console.log("      Flow: request -> 402 -> build CPI tx -> Crossmint signs ->");
console.log("      facilitator verifies (Path 2: inner instructions) -> settle\n");

const start = Date.now();

try {
  const response = await fetchWithPayer(`${SERVER_URL}/protected`);
  const elapsed = Date.now() - start;

  if (response.ok) {
    const body = await response.json();
    console.log("=== PAYMENT SUCCESSFUL ===\n");
    console.log(`  Status:      ${response.status}`);
    console.log(`  Time:        ${elapsed}ms`);
    console.log(`  Message:     ${body.message}`);
    console.log(`  Pay to:      ${body.payTo}`);
    console.log(`  Amount:      ${body.amount}`);
    console.log(`  Network:     ${body.network}`);
    console.log(`  Facilitator: ${body.facilitator}`);
    console.log("\n  Crossmint smart wallet -> CPI TransferChecked -> Path 2 verified!\n");
  } else {
    const text = await response.text();
    console.error(`=== PAYMENT FAILED ===\n`);
    console.error(`  Status: ${response.status}`);
    console.error(`  Time:   ${elapsed}ms`);
    console.error(`  Body:   ${text}\n`);
    process.exit(1);
  }
} catch (e) {
  const elapsed = Date.now() - start;
  console.error(`=== ERROR ===\n`);
  console.error(`  Time:  ${elapsed}ms`);
  console.error(`  Error: ${e.message}`);
  if (e.cause) console.error(`  Cause: ${JSON.stringify(e.cause)}`);
  console.error("");
  process.exit(1);
}

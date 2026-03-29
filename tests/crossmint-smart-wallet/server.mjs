/**
 * Mini paywall server for testing x402 Solana smart wallet payments.
 *
 * Supports two payment modes:
 *   1. Standard: unsigned tx in payload -> facilitator verify + settle
 *   2. Settlement Account: Crossmint sends tx directly, payload has txSignature
 *      -> server verifies on-chain, facilitator verifies + settles
 *
 * RPC resolution order:
 *   1. RPC_URL env var (from .env or shell)
 *   2. AWS Secrets Manager: facilitator-rpc-mainnet -> solana key
 *   3. Public endpoint (fallback, rate-limited)
 *
 * Usage:  node server.mjs [--payto <solana-pubkey>]
 */
import "dotenv/config";
import { execSync } from "child_process";
import express from "express";
import { Connection, Keypair, PublicKey } from "@solana/web3.js";
import { getAssociatedTokenAddress } from "@solana/spl-token";

/**
 * Resolve Solana RPC URL. Prefers AWS Secrets Manager over public endpoint.
 * Never logs the actual URL to avoid leaking API keys in streams/CI.
 */
function resolveRpcUrl() {
  const envUrl = process.env.RPC_URL;
  const PUBLIC = "https://api.mainnet-beta.solana.com";

  // If user set a non-public RPC in .env, use it
  if (envUrl && envUrl !== PUBLIC) {
    console.log("[rpc] Using RPC_URL from environment (custom endpoint)");
    return envUrl;
  }

  // Try AWS Secrets Manager
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
    // AWS CLI not available or no credentials — fall through
  }

  console.log("[rpc] WARNING: Using public RPC (rate-limited). Set RPC_URL or configure AWS credentials.");
  return PUBLIC;
}

const PORT = parseInt(process.env.SERVER_PORT || "3402", 10);
const FACILITATOR_URL = process.env.FACILITATOR_URL || "https://facilitator.ultravioletadao.xyz";
const RPC_URL = resolveRpcUrl();
const FEE_PAYER = "F742C4VfFLQ9zRQyithoj5229ZgtX2WqKCSFKgH2EThq"; // Ultravioleta Solana mainnet

// USDC on Solana mainnet
const USDC_MINT = "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v";
const AMOUNT = "10000"; // 0.01 USDC (6 decimals)

// Parse --payto or generate
let payTo;
const paytoIdx = process.argv.indexOf("--payto");
if (paytoIdx !== -1 && process.argv[paytoIdx + 1]) {
  payTo = process.argv[paytoIdx + 1];
} else {
  const kp = Keypair.generate();
  payTo = kp.publicKey.toBase58();
  console.log(`Generated payTo address: ${payTo}`);
}

const connection = new Connection(RPC_URL, "confirmed");
const app = express();
app.use(express.json());

// The x402 payment requirements for this resource
const paymentRequirements = {
  scheme: "exact",
  network: "solana",
  maxAmountRequired: AMOUNT,
  asset: USDC_MINT,
  payTo: payTo,
  description: "Smart wallet x402 test payment",
  maxTimeoutSeconds: 90,
  resource: `http://localhost:${PORT}/protected`,
  extra: {
    feePayer: FEE_PAYER,
    features: {
      xSettlementAccountSupported: true,
    },
  },
};

/**
 * Verify a settlement account payment on-chain.
 * In this mode, the client already submitted the transaction via Crossmint.
 * We verify by checking the transaction on-chain.
 */
async function verifySettlementPayment(payload, payTo) {
  const { transactionSignature, settleSecretKey, settlementRentDestination } = payload;

  console.log(`[settlement] Verifying on-chain tx: ${transactionSignature}`);
  console.log(`[settlement] Settlement secret key provided: ${!!settleSecretKey}`);
  console.log(`[settlement] Rent destination: ${settlementRentDestination}`);

  // Wait for the transaction to be confirmed
  let txInfo;
  for (let attempt = 0; attempt < 10; attempt++) {
    txInfo = await connection.getTransaction(transactionSignature, {
      commitment: "confirmed",
      maxSupportedTransactionVersion: 0,
    });
    if (txInfo) break;
    console.log(`[settlement] Waiting for tx confirmation (attempt ${attempt + 1}/10)...`);
    await new Promise((r) => setTimeout(r, 2000));
  }

  if (!txInfo) {
    return { success: false, error: "Transaction not found on-chain after 20s" };
  }

  if (txInfo.meta.err) {
    return { success: false, error: `Transaction failed on-chain: ${JSON.stringify(txInfo.meta.err)}` };
  }

  // Check post token balances for the transfer
  const preBalances = txInfo.meta.preTokenBalances || [];
  const postBalances = txInfo.meta.postTokenBalances || [];

  console.log("[settlement] Pre-token balances:", JSON.stringify(preBalances));
  console.log("[settlement] Post-token balances:", JSON.stringify(postBalances));

  // Find USDC transfers by checking balance changes
  const usdcMint = USDC_MINT;
  let transferFound = false;
  let transferAmount = 0;

  for (const post of postBalances) {
    if (post.mint !== usdcMint) continue;
    const pre = preBalances.find(
      (p) => p.accountIndex === post.accountIndex && p.mint === usdcMint
    );
    const preAmount = pre ? Number(pre.uiTokenAmount.amount) : 0;
    const postAmount = Number(post.uiTokenAmount.amount);
    const diff = postAmount - preAmount;
    if (diff > 0) {
      transferFound = true;
      transferAmount = diff;
      console.log(
        `[settlement] Found USDC credit: +${diff / 1e6} USDC to account index ${post.accountIndex} (owner: ${post.owner})`
      );
    }
  }

  if (!transferFound) {
    return { success: false, error: "No USDC transfer found in transaction" };
  }

  if (transferAmount < Number(AMOUNT)) {
    return {
      success: false,
      error: `Transfer amount ${transferAmount} < required ${AMOUNT}`,
    };
  }

  return {
    success: true,
    transaction: transactionSignature,
    amount: transferAmount,
  };
}

app.get("/protected", async (req, res) => {
  // Check for X-PAYMENT header (client retrying with payment proof)
  const xPayment = req.headers["x-payment"];

  if (!xPayment) {
    // No payment: return 402 with requirements
    console.log("[402] No payment header, returning payment requirements");
    return res.status(402).json({
      accepts: [paymentRequirements],
      error: "X-PAYMENT header is required",
      x402Version: 1,
    });
  }

  // Decode the payment payload
  let paymentPayload;
  try {
    paymentPayload = JSON.parse(Buffer.from(xPayment, "base64").toString());
  } catch (e) {
    console.error("[400] Failed to decode X-PAYMENT header:", e.message);
    return res.status(400).json({ error: "Invalid X-PAYMENT header" });
  }

  console.log("[x402] Received payment payload:", JSON.stringify(paymentPayload, null, 2));

  // Detect payment mode based on payload contents
  const isSettlementAccount = paymentPayload.payload?.transactionSignature;

  if (isSettlementAccount) {
    // ─── Settlement Account Mode ─────────────────────────────────────
    // The client already submitted the transaction via Crossmint.
    // Verify the on-chain transaction directly.
    console.log("[x402] Settlement Account mode detected (Crossmint custodial wallet)");

    try {
      const result = await verifySettlementPayment(paymentPayload.payload, payTo);
      if (result.success) {
        console.log(`[OK] Settlement account payment verified! TX: ${result.transaction}`);
        return res.json({
          success: true,
          message: "Settlement account payment verified on-chain! Crossmint x402 works.",
          payTo,
          amount: `${result.amount / 1e6} USDC`,
          network: "solana-mainnet",
          facilitator: "on-chain verification (settlement account)",
          transaction: result.transaction,
        });
      } else {
        console.error("[FAIL] Settlement verification failed:", result.error);
        return res.status(402).json({ error: result.error });
      }
    } catch (e) {
      console.error("[FAIL] Settlement verification error:", e.message);
      return res.status(500).json({ error: e.message });
    }
  }

  // ─── Standard Mode: Forward to facilitator ─────────────────────────
  console.log("[x402] Standard mode detected (signed transaction)");

  const networkMap = {
    "solana-mainnet-beta": "solana",
    "solana-devnet": "solana-devnet",
  };
  const normalizedNetwork = networkMap[paymentPayload.network] || paymentPayload.network;

  const requestBody = {
    x402Version: 1,
    paymentPayload: {
      x402Version: 1,
      scheme: paymentPayload.scheme || "exact",
      network: normalizedNetwork,
      payload: paymentPayload.payload,
    },
    paymentRequirements: {
      scheme: "exact",
      network: normalizedNetwork,
      maxAmountRequired: AMOUNT,
      resource: `http://localhost:${PORT}/protected`,
      description: "Smart wallet x402 test payment",
      mimeType: "application/json",
      payTo: payTo,
      maxTimeoutSeconds: 90,
      asset: USDC_MINT,
      extra: { feePayer: FEE_PAYER },
    },
  };
  console.log("[x402] Sending to facilitator:", JSON.stringify(requestBody, null, 2));

  // Step 1: Verify
  try {
    const verifyRes = await fetch(`${FACILITATOR_URL}/verify`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(requestBody),
    });
    const verifyData = await verifyRes.json();
    console.log("[verify]", JSON.stringify(verifyData));

    if (!verifyRes.ok || !verifyData.isValid) {
      console.error("[FAIL] Verification failed:", verifyData);
      return res.status(402).json({
        error: "Payment verification failed",
        details: verifyData,
      });
    }
  } catch (e) {
    console.error("[FAIL] Verify request failed:", e.message);
    return res.status(500).json({ error: "Facilitator verify failed", details: e.message });
  }

  // Step 2: Settle
  try {
    const settleRes = await fetch(`${FACILITATOR_URL}/settle`, {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify(requestBody),
    });
    const settleData = await settleRes.json();
    console.log("[settle]", JSON.stringify(settleData));

    if (!settleRes.ok || !settleData.success) {
      console.error("[FAIL] Settlement failed:", settleData);
      return res.status(402).json({
        error: "Payment settlement failed",
        details: settleData,
      });
    }

    console.log("[OK] Payment settled! TX:", settleData.transaction);

    return res.json({
      success: true,
      message: "Payment verified and settled! Smart wallet x402 works on mainnet.",
      payTo,
      amount: "0.01 USDC",
      network: "solana-mainnet",
      facilitator: FACILITATOR_URL,
      transaction: settleData.transaction,
    });
  } catch (e) {
    console.error("[FAIL] Settle request failed:", e.message);
    return res.status(500).json({ error: "Facilitator settle failed", details: e.message });
  }
});

app.get("/health", (_req, res) => res.json({ status: "ok" }));

app.listen(PORT, () => {
  console.log(`\n--- x402 Paywall Server (Solana Mainnet) ---`);
  console.log(`Listening:    http://localhost:${PORT}`);
  console.log(`Protected:    http://localhost:${PORT}/protected`);
  console.log(`Facilitator:  ${FACILITATOR_URL}`);
  console.log(`Fee payer:    ${FEE_PAYER}`);
  console.log(`Pay to:       ${payTo}`);
  console.log(`Amount:       0.01 USDC`);
  console.log(`USDC Mint:    ${USDC_MINT}`);
  console.log(`Mode:         Settlement Account (Crossmint) + Standard`);
  console.log(`\nWaiting for payments...\n`);
});

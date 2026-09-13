// PYUSD (Token-2022) on Solana: one real x402 `exact` payment through a facilitator.
//
// Builds the transaction the way a standard wallet does -- compute budget, an
// idempotent CreateATA for the payee when it has none, and a top-level Token-2022
// TransferChecked -- signs it as the payer, leaves the fee payer slot to the
// facilitator, then calls /verify and /settle and prints both balances around it.
//
//   node pay.mjs --network solana-devnet --facilitator http://127.0.0.1:18402 \
//     --payer ./payer.json --pay-to <address> [--amount 10000] [--rpc <url>] [--verify-only]
//
// --payer is a Solana CLI keypair file (JSON array of 64 bytes). It needs the
// PYUSD and, only if the payee has no PYUSD account yet, ~0.0021 SOL of rent for
// it. The facilitator pays the transaction fee. Amounts are raw units (6 decimals).
import {
  ComputeBudgetProgram, Connection, Keypair, PublicKey, Transaction,
} from "@solana/web3.js";
import {
  TOKEN_2022_PROGRAM_ID, createAssociatedTokenAccountIdempotentInstruction,
  createTransferCheckedInstruction, getAssociatedTokenAddressSync,
} from "@solana/spl-token";
import fs from "node:fs";
import { parseArgs } from "node:util";

// Mirrors PYUSD_SOLANA / PYUSD_SOLANA_DEVNET in src/network.rs.
const NETWORKS = {
  solana: {
    mint: "2b1kV6DkPAnxd5ixfnxCpjxmKwqjjaYmCZfHsFu24GXo",
    rpc: "https://api.mainnet-beta.solana.com",
  },
  "solana-devnet": {
    mint: "CXk2AMBfi3TwaEL2468s6zP8xq9NxTXjp9gjMgzeUynM",
    rpc: "https://api.devnet.solana.com",
  },
};

const { values: args } = parseArgs({
  options: {
    network: { type: "string", default: "solana-devnet" },
    facilitator: { type: "string", default: "http://127.0.0.1:8080" },
    payer: { type: "string" },
    "pay-to": { type: "string" },
    amount: { type: "string", default: "10000" },
    rpc: { type: "string" },
    "verify-only": { type: "boolean", default: false },
  },
});
const net = NETWORKS[args.network];
if (!net || !args.payer || !args["pay-to"]) {
  console.error("usage: node pay.mjs --network solana|solana-devnet --facilitator <url> --payer <keypair.json> --pay-to <address> [--amount 10000] [--rpc <url>] [--verify-only]");
  process.exit(2);
}

const conn = new Connection(args.rpc || net.rpc, "confirmed");
const mint = new PublicKey(net.mint);
const payer = Keypair.fromSecretKey(Uint8Array.from(JSON.parse(fs.readFileSync(args.payer, "utf8"))));
const payTo = new PublicKey(args["pay-to"]);
const amount = BigInt(args.amount);
const ata = (owner) => getAssociatedTokenAddressSync(mint, owner, false, TOKEN_2022_PROGRAM_ID);
const balance = async (owner) => {
  try { return (await conn.getTokenAccountBalance(ata(owner))).value.amount; } catch { return "no account"; }
};

const supported = await (await fetch(`${args.facilitator}/supported`)).json();
const kind = supported.kinds.find((k) => k.network === args.network && k.extra?.feePayer);
if (!kind) throw new Error(`${args.facilitator} does not serve ${args.network}`);
const feePayer = new PublicKey(kind.extra.feePayer);

console.log("network    ", args.network, "mint", mint.toBase58());
console.log("facilitator", args.facilitator, "feePayer", feePayer.toBase58());
console.log("payer      ", payer.publicKey.toBase58(), "payTo", payTo.toBase58(), "amount", amount.toString());
console.log("before      payer", await balance(payer.publicKey), "payTo", await balance(payTo));

// A FINALIZED blockhash: /verify simulates at `confirmed`, but /settle's
// sendTransaction preflight runs at the RPC client's default commitment, and a
// blockhash fetched at `confirmed` a moment earlier is answered "Blockhash not found".
const tx = new Transaction({ feePayer, recentBlockhash: (await conn.getLatestBlockhash("finalized")).blockhash });
tx.add(
  ComputeBudgetProgram.setComputeUnitLimit({ units: 200_000 }),
  ComputeBudgetProgram.setComputeUnitPrice({ microLamports: 1 }),
);
if (!(await conn.getAccountInfo(ata(payTo)))) {
  tx.add(createAssociatedTokenAccountIdempotentInstruction(
    payer.publicKey, ata(payTo), payTo, mint, TOKEN_2022_PROGRAM_ID,
  ));
}
tx.add(createTransferCheckedInstruction(
  ata(payer.publicKey), mint, ata(payTo), payer.publicKey, amount, 6, [], TOKEN_2022_PROGRAM_ID,
));
tx.partialSign(payer);

const body = JSON.stringify({
  x402Version: 1,
  paymentPayload: {
    x402Version: 1, scheme: "exact", network: args.network,
    payload: { transaction: tx.serialize({ requireAllSignatures: false }).toString("base64") },
  },
  paymentRequirements: {
    scheme: "exact", network: args.network, maxAmountRequired: amount.toString(),
    resource: "https://facilitator.ultravioletadao.xyz/pyusd-solana-e2e",
    description: "PYUSD Token-2022 e2e", mimeType: "application/json",
    payTo: payTo.toBase58(), maxTimeoutSeconds: 120, asset: mint.toBase58(),
  },
});
// The per-IP rate limiter keys on X-Forwarded-For, which the production load
// balancer always adds. A direct connection to a local facilitator has none and
// is answered 500 `rate_limit_key_unavailable`, so set it only for loopback.
const headers = { "content-type": "application/json" };
if (["127.0.0.1", "localhost", "[::1]"].includes(new URL(args.facilitator).hostname)) {
  headers["x-forwarded-for"] = "127.0.0.1";
}
for (const endpoint of args["verify-only"] ? ["verify"] : ["verify", "settle"]) {
  const res = await fetch(`${args.facilitator}/${endpoint}`, { method: "POST", headers, body });
  console.log(endpoint.padEnd(11), res.status, await res.text());
}
console.log("after       payer", await balance(payer.publicKey), "payTo", await balance(payTo));

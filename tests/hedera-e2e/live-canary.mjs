// Sends exactly one explicitly requested canary through a Rust facilitator.
// Keys are read from stdin, never argv, disk, fixtures or reports.
import assert from "node:assert/strict";
import { createServer } from "node:http";
import { PrivateKey } from "@hiero-ledger/sdk";
import { createClientHederaSigner } from "@x402/hedera";
import { ExactHederaScheme } from "@x402/hedera/exact/client";
import { x402Client, x402HTTPClient } from "@x402/core/client";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const config = JSON.parse(input);
input = "";
assert.equal(config.confirm, true);
assert(["hedera:testnet", "hedera:mainnet"].includes(config.network));
assert(BigInt(config.amount) > 0n && BigInt(config.amount) <= 1000000n, "bounded canary amount required");
assert.notEqual(config.payer, config.feePayer);
assert.notEqual(config.payTo, config.feePayer);
const origin = new URL(config.facilitator);
assert(origin.protocol === "https:" || ["127.0.0.1", "localhost"].includes(origin.hostname));
const post = async (path, body) => {
  const response = await fetch(new URL(path, origin), { method: "POST", headers: { "content-type": "application/json", ...(["127.0.0.1", "localhost"].includes(origin.hostname) ? { "x-forwarded-for": "127.0.0.1" } : {}) }, body: JSON.stringify(body), signal: AbortSignal.timeout(90000) });
  const result = await response.json();
  assert(response.ok, `${path} HTTP ${response.status}: ${JSON.stringify(result)}`);
  return result;
};
const requirements = { scheme: "exact", network: config.network, asset: config.asset,
  amount: config.amount, payTo: config.payTo, maxTimeoutSeconds: 180, extra: { feePayer: config.feePayer } };
let request, settlement;
const server = createServer(async (req, res) => {
  try {
    if (!req.headers["payment-signature"]) {
      const required = { x402Version: 2, resource: { url: `http://127.0.0.1:${server.address().port}/paid`, description: "Hedera native canary", mimeType: "application/json" }, accepts: [requirements] };
      res.writeHead(402, { "payment-required": Buffer.from(JSON.stringify(required)).toString("base64") }); res.end(); return;
    }
    const paymentPayload = JSON.parse(Buffer.from(req.headers["payment-signature"], "base64").toString());
    request = { x402Version: 2, paymentPayload, paymentRequirements: requirements };
    const verify = await post("/verify", request);
    assert.equal(verify.isValid, true, JSON.stringify(verify));
    assert.equal(verify.payer, config.payer);
    settlement = await post("/settle", request);
    assert.equal(settlement.success, true, JSON.stringify(settlement));
    assert.equal(settlement.payer, config.payer);
    assert.equal(settlement.network, config.network);
    res.writeHead(200, { "content-type": "application/json", "payment-response": Buffer.from(JSON.stringify(settlement)).toString("base64") });
    res.end(JSON.stringify({ paid: true }));
  } catch (error) { res.writeHead(500); res.end(JSON.stringify({ error: error.message })); }
});
await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
try {
  // The JS SDK's bundled mainnet address book can contain retired nodes.
  // Freeze the official client against a current node obtained over HTTPS;
  // the facilitator independently checks its own network's allowed node IDs.
  const mirror = config.network === "hedera:mainnet" ? "https://mainnet-public.mirrornode.hedera.com" : "https://testnet.mirrornode.hedera.com";
  const nodesResponse = await fetch(`${mirror}/api/v1/network/nodes?node.id=0`);
  assert(nodesResponse.ok);
  const nodes = await nodesResponse.json();
  const node = nodes.nodes.find(n => n.node_account_id === "0.0.3");
  const endpoint = node?.service_endpoints.find(e => e.port === 50211 && /^(\d{1,3}\.){3}\d{1,3}$/.test(e.ip_address_v4));
  assert(endpoint, "current consensus node 0.0.3 is unavailable");
  const signer = createClientHederaSigner(config.payer, PrivateKey.fromStringDer(config.privateKey), { network: config.network, nodeUrl: `${endpoint.ip_address_v4}:50211` });
  delete config.privateKey;
  const client = new x402HTTPClient(new x402Client().register(config.network, new ExactHederaScheme(signer)).setSpendControls({ allowedAssets: [{ network: config.network, asset: config.asset, maxAmountPerPayment: config.amount }] }));
  const url = `http://127.0.0.1:${server.address().port}/paid`;
  const challenge = await fetch(url);
  assert.equal(challenge.status, 402);
  const required = client.getPaymentRequiredResponse(name => challenge.headers.get(name));
  const payload = await client.createPaymentPayload(required);
  const paid = await fetch(url, { headers: client.encodePaymentSignatureHeader(payload), signal: AbortSignal.timeout(100000) });
  const paidBody = await paid.json();
  assert.equal(paid.status, 200, JSON.stringify(paidBody));
  const receipt = client.getPaymentSettleResponse(name => paid.headers.get(name));
  assert.equal(receipt.transaction, settlement.transaction);
  const retry = await post("/settle", request);
  assert.equal(retry.transaction, settlement.transaction);
  assert.equal(retry.success, true);
  const replay = await post("/verify", request);
  assert.equal(replay.isValid, false);
  console.log(JSON.stringify({ kind: "x402-native-canary", at: new Date().toISOString(), facilitator: config.facilitator, network: config.network,
    asset: config.asset, amount: config.amount, payer: config.payer, payTo: config.payTo, feePayer: config.feePayer,
    sdk: "@x402/hedera@2.26.0", challengeStatus: 402, paidStatus: 200, settlement, retrySameTransaction: true, replayRejected: true }));
} finally { await new Promise(resolve => server.close(resolve)); }

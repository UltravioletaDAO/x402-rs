// Explicit testnet-only HTS fixture. Never imports production keys.
import assert from "node:assert/strict";
import { Client, PrivateKey, Hbar, TokenCreateTransaction, TokenAssociateTransaction, TokenType, TokenSupplyType } from "@hiero-ledger/sdk";
let input = ""; for await (const chunk of process.stdin) input += chunk;
const c = JSON.parse(input); input = "";
assert.equal(c.confirmTestnet, true);
assert.equal(c.operatorId, "0.0.8511157");
assert.equal(c.treasuryId, "0.0.10576386"); assert.equal(c.merchantId, "0.0.10576387");
const client = Client.forTestnet().setOperator(c.operatorId, PrivateKey.fromStringDer(c.operatorKey));
client.setDefaultMaxTransactionFee(new Hbar(20));
try {
  let tx = new TokenCreateTransaction().setTokenName("Ultravioleta x402 canary FT").setTokenSymbol("X402TEST")
    .setDecimals(4).setInitialSupply(10000).setTreasuryAccountId(c.treasuryId)
    .setTokenType(TokenType.FungibleCommon).setSupplyType(TokenSupplyType.Finite).setMaxSupply(10000)
    .setMaxTransactionFee(new Hbar(20)).freezeWith(client);
  tx = await tx.sign(PrivateKey.fromStringDer(c.treasuryKey));
  console.log(JSON.stringify({ stage: "prepared", network: "hedera:testnet", operation: "create-test-ft", transaction: tx.transactionId.toString() }));
  const response = await tx.execute(client); const receipt = await response.getReceipt(client);
  assert.equal(receipt.status.toString(), "SUCCESS");
  const token = receipt.tokenId.toString();
  console.log(JSON.stringify({ stage: "confirmed", network: "hedera:testnet", operation: "create-test-ft", transaction: response.transactionId.toString(), token, decimals: 4 }));
  let associate = new TokenAssociateTransaction().setAccountId(c.merchantId).setTokenIds([token]).setMaxTransactionFee(new Hbar(1)).freezeWith(client);
  associate = await associate.sign(PrivateKey.fromStringDer(c.merchantKey));
  const associated = await associate.execute(client); const result = await associated.getReceipt(client);
  assert.equal(result.status.toString(), "SUCCESS");
  console.log(JSON.stringify({ stage: "confirmed", network: "hedera:testnet", operation: "associate-test-ft", transaction: associated.transactionId.toString(), account: c.merchantId, token }));
} finally { client.close(); }

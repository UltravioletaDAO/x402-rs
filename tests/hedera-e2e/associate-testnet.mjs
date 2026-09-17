import {Client, Hbar, PrivateKey, TokenAssociateTransaction} from "@hiero-ledger/sdk";
let input = ""; for await (const chunk of process.stdin) input += chunk;
const cfg = JSON.parse(input); input = "";
if (cfg.network !== "testnet" || cfg.tokenId !== "0.0.429274" || cfg.accounts.length > 2) throw new Error("testnet USDC association only");
const client = Client.forTestnet();
client.setOperator(cfg.sponsorId, PrivateKey.fromString(cfg.sponsorKey));
try {
  for (const account of cfg.accounts) {
    const response = await fetch(`https://testnet.mirrornode.hedera.com/api/v1/accounts/${account.id}/tokens?token.id=${cfg.tokenId}`);
    if (!response.ok) throw new Error("Mirror unavailable");
    const state = await response.json();
    if (state.tokens?.some(t => t.token_id === cfg.tokenId)) {
      console.log(JSON.stringify({accountId:account.id,tokenId:cfg.tokenId,alreadyAssociated:true})); continue;
    }
    const tx = await new TokenAssociateTransaction().setAccountId(account.id).setTokenIds([cfg.tokenId])
      .setMaxTransactionFee(Hbar.fromTinybars(100_000_000)).freezeWith(client).sign(PrivateKey.fromString(account.key));
    const submitted = await tx.execute(client);
    console.log(JSON.stringify({phase:"submitted", accountId:account.id, tokenId:cfg.tokenId, transactionId:submitted.transactionId.toString()}));
    const receipt = await submitted.getReceipt(client);
    console.log(JSON.stringify({phase:"confirmed", accountId:account.id, tokenId:cfg.tokenId, transactionId:submitted.transactionId.toString(), status:receipt.status.toString()}));
  }
} catch (error) {
  console.error(JSON.stringify({error:"association interrupted; reconcile submitted IDs before retrying",type:error?.name ?? "Error"})); process.exitCode = 1;
} finally { client.close(); }

// One-shot testnet account provisioning. Secrets arrive on stdin, never argv,
// environment logs or files. Only public receipts are written to stdout.
import {AccountCreateTransaction, Client, Hbar, PrivateKey, PublicKey, Status} from "@hiero-ledger/sdk";
let input = "";
for await (const chunk of process.stdin) input += chunk;
const cfg = JSON.parse(input);
input = "";
if (cfg.confirmTestnetProvisioning !== true || cfg.network !== "testnet") throw new Error("explicit testnet provisioning required");
if (!Array.isArray(cfg.accounts) || cfg.accounts.length > 3) throw new Error("at most three accounts");
const total = cfg.accounts.reduce((sum, a) => sum + a.initialTinybars, 0);
if (!Number.isSafeInteger(total) || total > 410_000_000) throw new Error("testnet principal cap exceeded");
const client = Client.forTestnet();
client.setOperator(cfg.operatorId, PrivateKey.fromString(cfg.operatorKey));
client.setDefaultMaxTransactionFee(Hbar.fromTinybars(100_000_000));
try {
  for (const account of cfg.accounts) {
    if (!Number.isSafeInteger(account.initialTinybars) || account.initialTinybars < 0 || account.existingAccountId) throw new Error("invalid or already provisioned account");
    const tx = await new AccountCreateTransaction()
      .setKeyWithoutAlias(PublicKey.fromString(account.publicKey))
      .setInitialBalance(Hbar.fromTinybars(account.initialTinybars))
      .setMaxAutomaticTokenAssociations(0)
      .setMaxTransactionFee(Hbar.fromTinybars(100_000_000))
      .execute(client);
    // Emit the ID before waiting: a timeout is recoverable without making a
    // second account and silently spending the allocation twice.
    console.log(JSON.stringify({phase:"submitted", label:account.label, transactionId:tx.transactionId.toString()}));
    const receipt = await tx.getReceipt(client);
    if (receipt.status !== Status.Success || !receipt.accountId) throw new Error("account creation not confirmed");
    console.log(JSON.stringify({phase:"confirmed", label:account.label, transactionId:tx.transactionId.toString(), accountId:receipt.accountId.toString(), initialTinybars:account.initialTinybars, publicKey:account.publicKey}));
  }
} catch (error) {
  // Avoid exception objects which may retain requests, configuration or keys.
  console.error(JSON.stringify({error:"testnet provisioning interrupted; reconcile submitted IDs before retrying",type:error?.name ?? "Error"}));
  process.exitCode = 1;
} finally {
  client.close(); cfg.operatorKey = undefined;
}

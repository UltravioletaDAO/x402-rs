// Offline generator of Hedera `exact` payment vectors for the x402-rs phase-0
// spike.
//
// Everything here runs with no network, no funds and no mainnet. The accounts
// are synthetic (0.0.1001 / 0.0.2002 / 0.0.3003) precisely so that no vector
// can be confused with a real facilitator's fee payer; the keys are derived
// from ASCII labels (see keys.mjs).
//
// The point of generating the vectors on the TypeScript side is that this is
// the side a real payer runs: `@x402/hedera` builds the transfer, freezes it
// and signs it, and the base64 it returns is exactly what lands in
// `payload.transaction`. A vector produced by the Rust SDK and then read back
// by the Rust SDK would prove nothing about interoperability.
import { writeFileSync, mkdirSync } from "node:fs";
import { createHash } from "node:crypto";
import {
  AccountId,
  Hbar,
  KeyList,
  TokenId,
  Timestamp,
  TransferTransaction,
  TransactionId,
} from "@hiero-ledger/sdk";
import { proto } from "@hiero-ledger/proto";
import { createClientHederaSigner } from "@x402/hedera";
import { LABELS, ed25519, ecdsa } from "./keys.mjs";

const SDK_VERSION = "2.85.0";
const PROTO_VERSION = "2.31.0";
const X402_HEDERA_VERSION = "2.26.0";

const SENDER = "0.0.1001";
const PAY_TO = "0.0.2002";
const FEE_PAYER = "0.0.3003";
const USDC_TESTNET = "0.0.429274"; // Mirror Node, 6 decimals, FUNGIBLE_COMMON
const NETWORK = "hedera:testnet";

// Fixed valid-start so the hand-built vectors are byte-reproducible. Hedera
// would reject this as expired on a live node; that is fine and deliberate --
// nothing here is ever submitted.
const VALID_START = new Timestamp(1757900000, 0);

const sha256 = (b) => createHash("sha256").update(b).digest("hex");
const b64 = (b) => Buffer.from(b).toString("base64");

/** Decode `Transaction.toBytes()` into its per-node variants, at the protobuf level. */
export function inspectVariants(bytes) {
  const list = proto.TransactionList.decode(bytes);
  return list.transactionList.map((entry, index) => {
    const signed = proto.SignedTransaction.decode(entry.signedTransactionBytes);
    const body = proto.TransactionBody.decode(signed.bodyBytes);
    return {
      index,
      bodySha256: sha256(signed.bodyBytes),
      bodyLength: signed.bodyBytes.length,
      nodeAccountId: accountIdToString(body.nodeAccountID),
      transactionId: txIdToString(body.transactionID),
      transactionFee: body.transactionFee?.toString() ?? null,
      validDurationSeconds: body.transactionValidDuration?.seconds?.toString() ?? null,
      memo: body.memo ?? "",
      bodyDataCase: bodyDataCase(body),
      transfers: describeTransfers(body),
      signatures: (signed.sigMap?.sigPair ?? []).map((pair) => ({
        publicKeyPrefixHex: Buffer.from(pair.pubKeyPrefix ?? []).toString("hex"),
        algorithm: pair.ed25519 ? "ed25519" : pair.ECDSASecp256k1 ? "ecdsa_secp256k1" : "unknown",
        signatureHex: Buffer.from(pair.ed25519 ?? pair.ECDSASecp256k1 ?? []).toString("hex"),
      })),
    };
  });
}

function bodyDataCase(body) {
  for (const k of [
    "cryptoTransfer", "cryptoCreateAccount", "contractCall", "scheduleCreate",
    "tokenMint", "cryptoApproveAllowance", "atomicBatch", "fileCreate",
  ]) {
    if (body[k]) return k;
  }
  return "other";
}

function accountIdToString(id) {
  if (!id) return null;
  return `${id.shardNum ?? 0}.${id.realmNum ?? 0}.${id.accountNum ?? 0}`;
}

function txIdToString(id) {
  if (!id) return null;
  const acc = accountIdToString(id.accountID);
  const s = id.transactionValidStart;
  return `${acc}@${s?.seconds ?? 0}.${String(s?.nanos ?? 0).padStart(9, "0")}`;
}

function describeTransfers(body) {
  const ct = body.cryptoTransfer;
  if (!ct) return null;
  return {
    hbar: (ct.transfers?.accountAmounts ?? []).map((a) => ({
      account: accountIdToString(a.accountID),
      amount: a.amount.toString(),
      isApproval: !!a.isApproval,
      hook: a.preTxAllowanceHook ? "preTxAllowanceHook" : a.prePostTxAllowanceHook ? "prePostTxAllowanceHook" : null,
    })),
    tokens: (ct.tokenTransfers ?? []).map((t) => ({
      token: t.token ? `${t.token.shardNum ?? 0}.${t.token.realmNum ?? 0}.${t.token.tokenNum ?? 0}` : null,
      expectedDecimals: t.expectedDecimals?.value ?? null,
      nftTransfers: (t.nftTransfers ?? []).length,
      transfers: (t.transfers ?? []).map((a) => ({
        account: accountIdToString(a.accountID),
        amount: a.amount.toString(),
        isApproval: !!a.isApproval,
        hook: a.preTxAllowanceHook ? "preTxAllowanceHook" : a.prePostTxAllowanceHook ? "prePostTxAllowanceHook" : null,
      })),
    })),
  };
}

function publicKeyRecord(label, key, type) {
  return {
    label,
    type,
    publicKeyDer: key.publicKey.toStringDer(),
    publicKeyRaw: key.publicKey.toStringRaw(),
  };
}

function vector(name, description, opts) {
  const bytes = opts.bytes;
  const variants = inspectVariants(bytes);
  return {
    name,
    description,
    generatedAt: new Date().toISOString(),
    generator: {
      "@hiero-ledger/sdk": SDK_VERSION,
      "@hiero-ledger/proto": PROTO_VERSION,
      "@x402/hedera": X402_HEDERA_VERSION,
      via: opts.via,
      reproducible: opts.reproducible !== false,
    },
    x402Version: 2,
    scheme: "exact",
    network: NETWORK,
    paymentRequirements: opts.paymentRequirements,
    keys: opts.keys,
    payload: { transaction: b64(bytes) },
    expected: {
      ...opts.expected,
      variantCount: variants.length,
      variants,
    },
    facilitatorMustCoSign: opts.facilitatorMustCoSign !== false,
    adversarial: opts.adversarial ?? null,
  };
}

function requirements({ asset, amount }) {
  return {
    scheme: "exact",
    network: NETWORK,
    asset,
    amount,
    payTo: PAY_TO,
    maxTimeoutSeconds: 60,
    resource: "https://example.test/resource",
    description: "phase-0 spike vector",
    mimeType: "application/json",
    extra: { feePayer: FEE_PAYER },
  };
}

async function buildFrozen({ asset, amount, sender, nodes, signWith }) {
  const tx = new TransferTransaction();
  if (asset === "0.0.0") {
    tx.addHbarTransfer(AccountId.fromString(sender), Hbar.fromTinybars(`-${amount}`));
    tx.addHbarTransfer(AccountId.fromString(PAY_TO), Hbar.fromTinybars(amount));
  } else {
    const tokenId = TokenId.fromString(asset);
    tx.addTokenTransfer(tokenId, AccountId.fromString(sender), -BigInt(amount));
    tx.addTokenTransfer(tokenId, AccountId.fromString(PAY_TO), BigInt(amount));
  }
  tx.setTransactionId(
    TransactionId.withValidStart(AccountId.fromString(FEE_PAYER), VALID_START),
  );
  tx.setNodeAccountIds(nodes.map((n) => AccountId.fromString(n)));
  tx.freeze();
  let signed = tx;
  for (const key of signWith) {
    signed = await signed.sign(key);
  }
  return signed.toBytes();
}

// --- adversarial rebuild helpers ------------------------------------------
//
// These take a legitimate multi-variant payload and rewrite ONE variant, which
// is the attack the plan calls out in 7.3 point 2: a facilitator that inspects
// only the first body signs a list whose other bodies say something else.

function rewriteVariant(bytes, index, mutateBody, resign) {
  const list = proto.TransactionList.decode(bytes);
  const entry = list.transactionList[index];
  const signed = proto.SignedTransaction.decode(entry.signedTransactionBytes);
  const body = proto.TransactionBody.decode(signed.bodyBytes);
  mutateBody(body);
  const newBodyBytes = proto.TransactionBody.encode(body).finish();
  const sigMap = resign
    ? { sigPair: resign(newBodyBytes) }
    : signed.sigMap;
  const newSigned = proto.SignedTransaction.encode({
    bodyBytes: newBodyBytes,
    sigMap,
  }).finish();
  list.transactionList[index] = { signedTransactionBytes: newSigned };
  return proto.TransactionList.encode(list).finish();
}

function rewriteAllVariants(bytes, mutateBody, resign) {
  const list = proto.TransactionList.decode(bytes);
  for (let i = 0; i < list.transactionList.length; i++) {
    const signed = proto.SignedTransaction.decode(list.transactionList[i].signedTransactionBytes);
    const body = proto.TransactionBody.decode(signed.bodyBytes);
    mutateBody(body);
    const newBodyBytes = proto.TransactionBody.encode(body).finish();
    const newSigned = proto.SignedTransaction.encode({
      bodyBytes: newBodyBytes,
      sigMap: resign ? { sigPair: resign(newBodyBytes) } : signed.sigMap,
    }).finish();
    list.transactionList[i] = { signedTransactionBytes: newSigned };
  }
  return proto.TransactionList.encode(list).finish();
}

function ed25519SigPair(key, bodyBytes) {
  return [{
    pubKeyPrefix: key.publicKey.toBytesRaw(),
    ed25519: key.sign(bodyBytes),
  }];
}

async function main() {
  mkdirSync("vectors", { recursive: true });

  const senderEd = ed25519(LABELS.senderEd25519);
  const senderEc = ecdsa(LABELS.senderEcdsa);
  const facilitator = ed25519(LABELS.facilitator);
  const t1 = ed25519(LABELS.threshold1);
  const t2 = ed25519(LABELS.threshold2);
  const t3 = ecdsa(LABELS.threshold3);
  const outsider = ed25519(LABELS.outsider);

  const written = [];
  const write = (v) => {
    const path = `vectors/${v.name}.json`;
    writeFileSync(path, JSON.stringify(v, null, 2) + "\n");
    written.push({ name: v.name, variants: v.expected.variantCount, path });
  };

  // 1. The real client path: @x402/hedera builds, freezes against
  //    Client.forTestnet() and signs. Node set and transaction id come from
  //    the SDK, not from us, so this one is NOT byte-reproducible.
  {
    const signer = createClientHederaSigner(SENDER, senderEd, { network: NETWORK });
    const reqs = requirements({ asset: "0.0.0", amount: "1000000" });
    const base64 = await signer.createPartiallySignedTransferTransaction(reqs);
    write(vector(
      "01-hbar-ed25519-official-client",
      "HBAR transfer built by @x402/hedera's own client signer against Client.forTestnet(): the exact shape a real payer emits, including whatever node set the SDK chose.",
      {
        bytes: Buffer.from(base64, "base64"),
        via: "@x402/hedera createClientHederaSigner().createPartiallySignedTransferTransaction",
        reproducible: false,
        paymentRequirements: reqs,
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          senderAccountKey: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount: "1000000",
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
        },
      },
    ));
  }

  // 2. HBAR, Ed25519, four nodes, fixed valid start: the reproducible baseline.
  {
    const amount = "1000000";
    const bytes = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5", "0.0.6"], signWith: [senderEd],
    });
    write(vector(
      "02-hbar-ed25519-multinode",
      "HBAR transfer frozen over four consensus nodes and signed by an Ed25519 sender. Reproducible byte for byte.",
      {
        bytes, via: "@hiero-ledger/sdk TransferTransaction.freeze()",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
          outsider: publicKeyRecord(LABELS.outsider, outsider, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
        },
      },
    ));
  }

  // 3. HTS USDC, ECDSA secp256k1 sender, three nodes.
  {
    const amount = "150000"; // 0.15 USDC at 6 decimals
    const bytes = await buildFrozen({
      asset: USDC_TESTNET, amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5"], signWith: [senderEc],
    });
    write(vector(
      "03-hts-usdc-ecdsa-multinode",
      "HTS transfer of testnet USDC (0.0.429274, 6 decimals) signed by an ECDSA secp256k1 sender, frozen over three nodes.",
      {
        bytes, via: "@hiero-ledger/sdk TransferTransaction.freeze()",
        paymentRequirements: requirements({ asset: USDC_TESTNET, amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEcdsa, senderEc, "ECDSA_SECP256K1"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: USDC_TESTNET, amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
        },
      },
    ));
  }

  // 4. Threshold 2-of-3 (Ed25519, Ed25519, ECDSA); only two of the three sign.
  {
    const amount = "2500000";
    const keyList = KeyList.of(t1.publicKey, t2.publicKey, t3.publicKey).setThreshold(2);
    const bytes = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4"], signWith: [t1, t3],
    });
    write(vector(
      "04-hbar-threshold-2of3",
      "Sender account governed by a 2-of-3 threshold key (Ed25519, Ed25519, ECDSA). Signed by key 1 and key 3 only: a correct verifier must accept, and must not require the missing key.",
      {
        bytes, via: "@hiero-ledger/sdk TransferTransaction.freeze()",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          senderAccountKey: {
            type: "THRESHOLD",
            threshold: 2,
            protobufHex: Buffer.from(proto.Key.encode(keyList._toProtobufKey()).finish()).toString("hex"),
            members: [
              publicKeyRecord(LABELS.threshold1, t1, "ED25519"),
              publicKeyRecord(LABELS.threshold2, t2, "ED25519"),
              publicKeyRecord(LABELS.threshold3, t3, "ECDSA_SECP256K1"),
            ],
            signedBy: [LABELS.threshold1, LABELS.threshold3],
          },
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 2,
        },
      },
    ));
  }

  // 5. Plain KeyList (no threshold => every member required); both members sign.
  {
    const amount = "777000";
    const keyList = KeyList.of(t1.publicKey, t2.publicKey);
    const bytes = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5"], signWith: [t1, t2],
    });
    write(vector(
      "05-hbar-keylist-all",
      "Sender account governed by a plain KeyList with no threshold, so every member must sign. Both members do.",
      {
        bytes, via: "@hiero-ledger/sdk TransferTransaction.freeze()",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          senderAccountKey: {
            type: "KEYLIST",
            threshold: null,
            protobufHex: Buffer.from(proto.Key.encode(keyList._toProtobufKey()).finish()).toString("hex"),
            members: [
              publicKeyRecord(LABELS.threshold1, t1, "ED25519"),
              publicKeyRecord(LABELS.threshold2, t2, "ED25519"),
            ],
            signedBy: [LABELS.threshold1, LABELS.threshold2],
          },
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 2,
        },
      },
    ));
  }

  // 6. ADVERSARIAL. Variant 1 pays payTo; variant 2 pays an attacker account
  //    and is CORRECTLY SIGNED by the sender over that other body. Every
  //    variant verifies on its own; the list does not say one thing.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5"], signWith: [senderEd],
    });
    const attacker = "0.0.666666";
    const bytes = rewriteVariant(base, 1, (body) => {
      for (const aa of body.cryptoTransfer.transfers.accountAmounts) {
        if (accountIdToString(aa.accountID) === PAY_TO) {
          aa.accountID = AccountId.fromString(attacker)._toProtobuf();
        }
      }
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "06-adversarial-second-variant-repointed",
      "Variant 0 and 2 pay payTo; variant 1 pays an attacker account and carries a VALID sender signature over that body. A facilitator that inspects only the first body and then co-signs the whole list hands the attacker a submittable transaction.",
      {
        bytes, via: "@hiero-ledger/proto hand-rewrite of variant 1",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
          divergentVariantIndex: 1,
          divergentPayTo: attacker,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "divergent_body_valid_signature",
          mustBeRejectedBecause: "not every variant expresses the same intent",
        },
      },
    ));
  }

  // 7. ADVERSARIAL. Same rewrite, signature left over the ORIGINAL body, so
  //    variant 1's signature does not verify against variant 1's bytes.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5"], signWith: [senderEd],
    });
    const bytes = rewriteVariant(base, 1, (body) => {
      body.cryptoTransfer.transfers.accountAmounts[0].amount =
        body.cryptoTransfer.transfers.accountAmounts[0].amount.multiply
          ? body.cryptoTransfer.transfers.accountAmounts[0].amount.multiply(1000)
          : -1000000000;
      body.cryptoTransfer.transfers.accountAmounts[1].amount = 1000000000;
    }, null);
    write(vector(
      "07-adversarial-second-variant-stale-signature",
      "Variant 1's amount is inflated 1000x and the sender signature is left over the original body, so it no longer verifies against the bytes it accompanies.",
      {
        bytes, via: "@hiero-ledger/proto hand-rewrite of variant 1",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: false,
          signaturesPerVariantBeforeCoSign: 1,
          divergentVariantIndex: 1,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "divergent_body_invalid_signature",
          mustBeRejectedBecause: "variant 1 carries a signature over different bytes",
        },
      },
    ));
  }

  // 8. ADVERSARIAL. Single variant, signed by a key that is not the sender's.
  {
    const amount = "1000000";
    const bytes = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3"], signWith: [outsider],
    });
    write(vector(
      "08-adversarial-wrong-signer",
      "Well-formed single-variant HBAR transfer signed by a key that does not control the debited account.",
      {
        bytes, via: "@hiero-ledger/sdk TransferTransaction.freeze()",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          actualSigner: publicKeyRecord(LABELS.outsider, outsider, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: false,
          signaturesPerVariantBeforeCoSign: 1,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "signature_from_unrelated_key",
          mustBeRejectedBecause: "the debited account did not authorize this body",
        },
      },
    ));
  }

  // 9. ADVERSARIAL. Variant 1 carries a DIFFERENT transactionId, correctly
  //    signed. hiero-sdk 0.45.0 compares bodies with `pb_transaction_body_eq`,
  //    which explicitly skips `transaction_id` and `node_account_id`, so this
  //    one is expected to slip past the SDK's own equality check.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5"], signWith: [senderEd],
    });
    const otherStart = { seconds: 1757900777, nanos: 0 };
    const bytes = rewriteVariant(base, 1, (body) => {
      body.transactionID.transactionValidStart = otherStart;
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "09-adversarial-second-variant-other-transaction-id",
      "Variant 1 is identical except for its transactionId, and is correctly signed over that body. The list therefore contains two DIFFERENT payments that the sender authorized.",
      {
        bytes, via: "@hiero-ledger/proto hand-rewrite of variant 1",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
          divergentVariantIndex: 1,
          divergentField: "transactionID.transactionValidStart",
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "divergent_transaction_id",
          mustBeRejectedBecause: "one payload must mean exactly one payment",
        },
      },
    ));
  }

  // 10. ADVERSARIAL. Variant 0 carries a sigPair with an EMPTY pubKeyPrefix and
  //     a junk signature. An empty prefix is a prefix of every public key.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4"], signWith: [senderEd],
    });
    const list = proto.TransactionList.decode(base);
    const signed = proto.SignedTransaction.decode(list.transactionList[0].signedTransactionBytes);
    signed.sigMap.sigPair.unshift({
      pubKeyPrefix: new Uint8Array(0),
      ed25519: new Uint8Array(64),
    });
    list.transactionList[0] = {
      signedTransactionBytes: proto.SignedTransaction.encode(signed).finish(),
    };
    const bytes = proto.TransactionList.encode(list).finish();
    write(vector(
      "10-adversarial-empty-pubkey-prefix",
      "Variant 0 carries a sigPair whose pubKeyPrefix is empty, which is a prefix of every public key, and whose signature is 64 zero bytes.",
      {
        bytes, via: "@hiero-ledger/proto hand-rewrite of variant 0",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 2,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "empty_public_key_prefix",
          mustBeRejectedBecause: "an empty prefix matches every key and can suppress the co-signature",
        },
      },
    ));
  }

  // 11. ADVERSARIAL. The sender's debit carries isApproval=true, i.e. spend from
  //     an allowance rather than from the sender's own authority. Present in the
  //     protobuf on both sides; `get_hbar_transfers()` in hiero-sdk returns a
  //     HashMap<AccountId, Hbar> and has nowhere to put this flag.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4"], signWith: [senderEd],
    });
    const bytes = rewriteAllVariants(base, (body) => {
      for (const aa of body.cryptoTransfer.transfers.accountAmounts) {
        if (accountIdToString(aa.accountID) === SENDER) aa.isApproval = true;
      }
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "11-adversarial-is-approval-debit",
      "Every variant debits the sender with isApproval=true: an allowance spend, not the sender paying. Signatures are valid over these bodies.",
      {
        bytes, via: "@hiero-ledger/proto rewrite of every variant",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "approved_allowance_debit",
          mustBeRejectedBecause: "plan 7.3 point 4 rejects isApproved transfers",
        },
      },
    ));
  }

  // 12. ADVERSARIAL. A legitimate-looking USDC transfer that also drags an NFT
  //     transfer along in the same body.
  {
    const amount = "150000";
    const base = await buildFrozen({
      asset: USDC_TESTNET, amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4"], signWith: [senderEd],
    });
    const nftToken = "0.0.777777";
    const bytes = rewriteAllVariants(base, (body) => {
      body.cryptoTransfer.tokenTransfers.push({
        token: TokenId.fromString(nftToken)._toProtobuf(),
        transfers: [],
        nftTransfers: [{
          senderAccountID: AccountId.fromString(SENDER)._toProtobuf(),
          receiverAccountID: AccountId.fromString("0.0.666666")._toProtobuf(),
          serialNumber: 42,
          isApproval: false,
        }],
      });
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "12-adversarial-nft-rider",
      "A USDC transfer that also moves an NFT to a third account in the same body.",
      {
        bytes, via: "@hiero-ledger/proto rewrite of every variant",
        paymentRequirements: requirements({ asset: USDC_TESTNET, amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: USDC_TESTNET, amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
          riderNftToken: nftToken,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "nft_rider",
          mustBeRejectedBecause: "plan 7.3 point 4 rejects NFT transfers",
        },
      },
    ));
  }

  // 13. ADVERSARIAL. The sender's debit carries a pre-transaction allowance hook.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4"], signWith: [senderEd],
    });
    const bytes = rewriteAllVariants(base, (body) => {
      for (const aa of body.cryptoTransfer.transfers.accountAmounts) {
        if (accountIdToString(aa.accountID) === SENDER) {
          aa.preTxAllowanceHook = { hookId: 7 };
        }
      }
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "13-adversarial-allowance-hook",
      "Every variant attaches a preTxAllowanceHook to the sender's debit, so a contract runs before the transfer.",
      {
        bytes, via: "@hiero-ledger/proto rewrite of every variant",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "allowance_hook",
          mustBeRejectedBecause: "plan 7.3 point 4 rejects hooks",
        },
      },
    ));
  }

  // 14. ADVERSARIAL. Two variants claim the SAME consensus node.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4", "0.0.5"], signWith: [senderEd],
    });
    const bytes = rewriteVariant(base, 1, (body) => {
      body.nodeAccountID = AccountId.fromString("0.0.3")._toProtobuf();
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "14-adversarial-duplicate-node",
      "Variants 0 and 1 both name node 0.0.3, correctly signed. Two submittable copies of the same payment aimed at one node.",
      {
        bytes, via: "@hiero-ledger/proto hand-rewrite of variant 1",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
          divergentVariantIndex: 1,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "duplicate_node_account_id",
          mustBeRejectedBecause: "one variant per node, and only nodes of the configured network",
        },
      },
    ));
  }

  // 15. ADVERSARIAL. The fee payer is also debited: the sponsor pays the
  //     network fee AND part of the principal.
  {
    const amount = "1000000";
    const base = await buildFrozen({
      asset: "0.0.0", amount, sender: SENDER,
      nodes: ["0.0.3", "0.0.4"], signWith: [senderEd],
    });
    const bytes = rewriteAllVariants(base, (body) => {
      const aas = body.cryptoTransfer.transfers.accountAmounts;
      for (const aa of aas) {
        if (accountIdToString(aa.accountID) === SENDER) aa.amount = -600000;
      }
      aas.push({
        accountID: AccountId.fromString(FEE_PAYER)._toProtobuf(),
        amount: -400000,
        isApproval: false,
      });
    }, (newBody) => ed25519SigPair(senderEd, newBody));
    write(vector(
      "15-adversarial-fee-payer-debited",
      "payTo still receives the full amount, but 40% of it comes out of the facilitator's own sponsoring account.",
      {
        bytes, via: "@hiero-ledger/proto rewrite of every variant",
        paymentRequirements: requirements({ asset: "0.0.0", amount }),
        keys: {
          sender: publicKeyRecord(LABELS.senderEd25519, senderEd, "ED25519"),
          facilitator: publicKeyRecord(LABELS.facilitator, facilitator, "ED25519"),
        },
        expected: {
          senderAccountId: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER,
          asset: "0.0.0", amount,
          senderSignsEveryVariant: true,
          signaturesPerVariantBeforeCoSign: 1,
        },
        facilitatorMustCoSign: false,
        adversarial: {
          kind: "sponsor_debited",
          mustBeRejectedBecause: "plan 7.3 point 6 rejects every negative entry for the sponsor",
        },
      },
    ));
  }

  writeFileSync(
    "vectors/index.json",
    JSON.stringify({
      generatedAt: new Date().toISOString(),
      versions: {
        "@hiero-ledger/sdk": SDK_VERSION,
        "@hiero-ledger/proto": PROTO_VERSION,
        "@x402/hedera": X402_HEDERA_VERSION,
      },
      accounts: { sender: SENDER, payTo: PAY_TO, feePayer: FEE_PAYER, usdcTestnet: USDC_TESTNET },
      vectors: written,
    }, null, 2) + "\n",
  );

  for (const w of written) console.log(`${w.name.padEnd(48)} variants=${w.variants}`);
}

main().then(() => process.exit(0)).catch((e) => { console.error(e); process.exit(1); });

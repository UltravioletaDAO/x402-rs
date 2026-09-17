// Deterministic test keys for the phase-0 Hedera spike.
//
// Nothing here is ever funded and nothing here touches mainnet. Keys are
// DERIVED from ASCII labels via SHA-256 rather than stored, for two reasons:
// the vectors stay reproducible from the repository alone, and no private key
// literal is ever written to a file the repository tracks (the pre-commit hook
// blocks 0x + 64 hex, and so it should).
//
// The Rust side derives the same keys from the same labels, which is also what
// makes the cross-SDK comparison meaningful: both sides start from identical
// key material without either one handing it to the other -- see the note on
// fromBytesED25519 below for the trap that sits on exactly that assumption.
import { createHash } from "node:crypto";
import { PrivateKey } from "@hiero-ledger/sdk";

export const LABELS = {
  senderEd25519: "x402-rs/hedera-spike/v1/ed25519/sender",
  senderEcdsa: "x402-rs/hedera-spike/v1/ecdsa/sender",
  facilitator: "x402-rs/hedera-spike/v1/ed25519/facilitator",
  facilitatorEcdsa: "x402-rs/hedera-spike/v1/ecdsa/facilitator",
  threshold1: "x402-rs/hedera-spike/v1/ed25519/threshold-1",
  threshold2: "x402-rs/hedera-spike/v1/ed25519/threshold-2",
  threshold3: "x402-rs/hedera-spike/v1/ecdsa/threshold-3",
  outsider: "x402-rs/hedera-spike/v1/ed25519/outsider",
};

export function seed(label) {
  return createHash("sha256").update(label, "utf8").digest();
}

// fromBytesED25519, NOT fromSeedED25519. They are different functions: the
// `fromSeed*` family runs an HMAC-SHA512 derivation over the input, while the
// Rust SDK's `PrivateKey::from_bytes_ed25519` takes the 32 bytes as the key
// itself. Feeding both the same seed yields two different accounts, which is
// exactly what `both_sdks_derive_the_same_keys_from_the_same_labels` caught.
export function ed25519(label) {
  return PrivateKey.fromBytesED25519(seed(label));
}

export function ecdsa(label) {
  return PrivateKey.fromBytesECDSA(seed(label));
}

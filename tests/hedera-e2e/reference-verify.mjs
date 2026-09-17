// Runs the OFFICIAL facilitator scheme (@x402/hedera 2.26.0) over the same
// vectors, so the comparison is against the reference implementation rather
// than against a reading of it.
//
// The payer-signature check is stubbed to `ok`. That is not a favour to the
// reference: the Rust spike already verified, against the frozen bodies, that
// the payer really did sign every one of these. Stubbing it removes the only
// step that needs a Mirror Node, and leaves the reference's own inspection of
// the transaction as the thing under test.
import { readFileSync } from "node:fs";
import { ExactHederaScheme } from "@x402/hedera/exact/facilitator";
import { vectorPaths } from "./lib-vectors.mjs";

const signer = {
  getAddresses: () => ["0.0.3003"],
  verifyPayerSignature: async () => ({ ok: true }),
  // Same reasoning for the preflight: it reads account existence, balance and
  // token association from a Mirror Node. The spike makes no network calls, so
  // it answers `ok` and leaves the reference's transaction inspection exposed.
  preflightTransfer: async () => ({ ok: true }),
  signAndSubmitTransaction: async () => {
    throw new Error("the spike never submits anything");
  },
};

const scheme = new ExactHederaScheme(signer);

for (const path of vectorPaths()) {
  const v = JSON.parse(readFileSync(path, "utf8"));
  // v2 envelope: `accepted` is a copy of the requirements, and the reference
  // refuses the request outright if the two differ in asset, amount, payTo,
  // maxTimeoutSeconds or extra.feePayer.
  const payload = {
    x402Version: v.x402Version,
    scheme: v.scheme,
    network: v.network,
    accepted: v.paymentRequirements,
    payload: v.payload,
  };
  let out;
  try {
    out = await scheme.verify(payload, v.paymentRequirements);
  } catch (e) {
    out = { isValid: false, invalidReason: `threw: ${e.message}` };
  }
  const mark = out.isValid ? "ACCEPTS" : "refuses";
  const why = out.isValid ? `payer=${out.payer}` : out.invalidReason;
  // The one place the stub changes the answer. Everywhere else the payer
  // really did sign, and the preflight only ever sees {payer, payTo, asset,
  // amount, network} -- never the transaction -- so it could not have caught a
  // rider, an allowance debit or a hook either.
  const caveat = v.name.startsWith("08-") && out.isValid
    ? "   <- artefact of the stubbed signature check, not a finding"
    : "";
  console.log(`${v.name.padEnd(52)} ${mark}  ${why}${caveat}`);
}

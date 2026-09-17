import assert from "node:assert/strict";
import { PrivateKey } from "@hiero-ledger/sdk";
import { createClientHederaSigner } from "@x402/hedera";
let input="";for await(const chunk of process.stdin)input+=chunk;
const c=JSON.parse(input);input="";assert.equal(c.confirmTestnet,true);
const requirements={scheme:"exact",network:"hedera:testnet",asset:"0.0.0",amount:"10000",payTo:c.payTo,maxTimeoutSeconds:180,extra:{feePayer:c.feePayer}};
const signer=createClientHederaSigner(c.payer,PrivateKey.fromStringDer(c.privateKey),{network:"hedera:testnet"});
const transaction=await signer.createPartiallySignedTransferTransaction(requirements);
console.log(JSON.stringify({x402Version:2,paymentPayload:{x402Version:2,accepted:requirements,payload:{transaction}},paymentRequirements:requirements}));

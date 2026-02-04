#!/usr/bin/env python3
"""
Verify that our nonce computation matches the x402r SDK.

Reference from x402r-scheme/packages/evm/src/shared/nonce.ts:

    const encoded = encodeAbiParameters(
        [
            { name: 'chainId', type: 'uint256' },
            { name: 'escrow', type: 'address' },
            { name: 'paymentInfo', type: 'tuple', components: PAYMENT_INFO_COMPONENTS },
        ],
        [BigInt(chainId), escrowAddress, paymentInfoWithZeroPayer]
    );
    return keccak256(encoded);

PAYMENT_INFO_COMPONENTS from constants.ts:
    { name: 'operator', type: 'address' },
    { name: 'payer', type: 'address' },
    { name: 'receiver', type: 'address' },
    { name: 'token', type: 'address' },
    { name: 'maxAmount', type: 'uint120' },
    { name: 'preApprovalExpiry', type: 'uint48' },
    { name: 'authorizationExpiry', type: 'uint48' },
    { name: 'refundExpiry', type: 'uint48' },
    { name: 'minFeeBps', type: 'uint16' },
    { name: 'maxFeeBps', type: 'uint16' },
    { name: 'feeReceiver', type: 'address' },
    { name: 'salt', type: 'uint256' },
"""

from eth_abi import encode
from web3 import Web3

# Test data matching the SDK example
CHAIN_ID = 8453
ESCROW_ADDRESS = "0x320a3c35F131E5D2Fb36af56345726B298936037"
ZERO_ADDRESS = "0x0000000000000000000000000000000000000000"

# Test payment info
PAYMENT_INFO = {
    "operator": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "payer": ZERO_ADDRESS,  # Zero for nonce computation
    "receiver": "0xD3868E1eD738CED6945A574a7c769433BeD5d474",
    "token": "0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913",
    "maxAmount": 10000,
    "preApprovalExpiry": 281474976710655,  # MAX_UINT48
    "authorizationExpiry": 281474976710655,
    "refundExpiry": 281474976710655,
    "minFeeBps": 0,
    "maxFeeBps": 100,
    "feeReceiver": "0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838",
    "salt": 0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef,
}


def compute_nonce():
    """Compute nonce exactly as x402r SDK does."""

    # Build the PaymentInfo tuple
    payment_info_tuple = (
        Web3.to_checksum_address(PAYMENT_INFO["operator"]),
        Web3.to_checksum_address(PAYMENT_INFO["payer"]),
        Web3.to_checksum_address(PAYMENT_INFO["receiver"]),
        Web3.to_checksum_address(PAYMENT_INFO["token"]),
        PAYMENT_INFO["maxAmount"],
        PAYMENT_INFO["preApprovalExpiry"],
        PAYMENT_INFO["authorizationExpiry"],
        PAYMENT_INFO["refundExpiry"],
        PAYMENT_INFO["minFeeBps"],
        PAYMENT_INFO["maxFeeBps"],
        Web3.to_checksum_address(PAYMENT_INFO["feeReceiver"]),
        PAYMENT_INFO["salt"],
    )

    # ABI encode (uint256 chainId, address escrow, tuple paymentInfo)
    encoded = encode(
        [
            "uint256",  # chainId
            "address",  # escrow
            "(address,address,address,address,uint120,uint48,uint48,uint48,uint16,uint16,address,uint256)",  # PaymentInfo tuple
        ],
        [
            CHAIN_ID,
            Web3.to_checksum_address(ESCROW_ADDRESS),
            payment_info_tuple,
        ]
    )

    # keccak256
    nonce = Web3.keccak(encoded)

    return nonce


def main():
    print("=== Nonce Computation Verification ===\n")

    print("Input Parameters:")
    print(f"  chainId: {CHAIN_ID}")
    print(f"  escrow: {ESCROW_ADDRESS}")
    print(f"  operator: {PAYMENT_INFO['operator']}")
    print(f"  payer: {PAYMENT_INFO['payer']} (ZERO)")
    print(f"  receiver: {PAYMENT_INFO['receiver']}")
    print(f"  token: {PAYMENT_INFO['token']}")
    print(f"  maxAmount: {PAYMENT_INFO['maxAmount']}")
    print(f"  preApprovalExpiry: {PAYMENT_INFO['preApprovalExpiry']}")
    print(f"  authorizationExpiry: {PAYMENT_INFO['authorizationExpiry']}")
    print(f"  refundExpiry: {PAYMENT_INFO['refundExpiry']}")
    print(f"  minFeeBps: {PAYMENT_INFO['minFeeBps']}")
    print(f"  maxFeeBps: {PAYMENT_INFO['maxFeeBps']}")
    print(f"  feeReceiver: {PAYMENT_INFO['feeReceiver']}")
    print(f"  salt: {hex(PAYMENT_INFO['salt'])}")

    nonce = compute_nonce()

    print(f"\n=== Result ===")
    print(f"Nonce: 0x{nonce.hex()}")

    # Expected result from running the JS code would go here
    print("\n=== To verify, run this in Node.js with x402r-scheme ===")
    print("""
const { encodeAbiParameters, keccak256 } = require('viem');
const PAYMENT_INFO_COMPONENTS = [
  { name: 'operator', type: 'address' },
  { name: 'payer', type: 'address' },
  { name: 'receiver', type: 'address' },
  { name: 'token', type: 'address' },
  { name: 'maxAmount', type: 'uint120' },
  { name: 'preApprovalExpiry', type: 'uint48' },
  { name: 'authorizationExpiry', type: 'uint48' },
  { name: 'refundExpiry', type: 'uint48' },
  { name: 'minFeeBps', type: 'uint16' },
  { name: 'maxFeeBps', type: 'uint16' },
  { name: 'feeReceiver', type: 'address' },
  { name: 'salt', type: 'uint256' },
];

const encoded = encodeAbiParameters(
  [
    { name: 'chainId', type: 'uint256' },
    { name: 'escrow', type: 'address' },
    { name: 'paymentInfo', type: 'tuple', components: PAYMENT_INFO_COMPONENTS },
  ],
  [
    BigInt(8453),
    '0x320a3c35F131E5D2Fb36af56345726B298936037',
    {
      operator: '0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838',
      payer: '0x0000000000000000000000000000000000000000',
      receiver: '0xD3868E1eD738CED6945A574a7c769433BeD5d474',
      token: '0x833589fCD6eDb6E08f4c7C32D4f71b54bdA02913',
      maxAmount: BigInt(10000),
      preApprovalExpiry: 281474976710655,
      authorizationExpiry: 281474976710655,
      refundExpiry: 281474976710655,
      minFeeBps: 0,
      maxFeeBps: 100,
      feeReceiver: '0xD979dBfBdA5f4b16AAF60Eaab32A44f352076838',
      salt: BigInt('0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef'),
    },
  ]
);

console.log('Nonce:', keccak256(encoded));
    """)


if __name__ == "__main__":
    main()

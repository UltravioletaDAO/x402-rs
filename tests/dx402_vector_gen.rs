//! Emits a DX402 sealed blob from fixed inputs, so other implementations can be
//! checked against this one rather than against themselves.
//!
//! Run with: `cargo test --test dx402_vector_gen -- --nocapture emit_vectors`
//!
//! This exists because three fabricated SHA-256 variants of ERC-8004 SEAL v1
//! passed CI for months by only ever being compared to their own output. A
//! vector is only worth anything if a second, independent implementation agrees.

use x402_rs::dx402::envelope::{seal, PayerPublicKey};
use x402_rs::dx402::pubkey;

#[test]
fn emit_vectors() {
    let payment_id = b"0x1111111111111111111111111111111111111111111111111111111111111111";
    let body = b"the paid response that must outlive the session";

    // secp256k1 (EVM payer)
    let sk = k256::SecretKey::from_slice(&[0x42u8; 32]).unwrap();
    let sealed = seal(
        body,
        &PayerPublicKey::Secp256k1(Box::new(sk.public_key())),
        payment_id,
    )
    .unwrap();
    println!("SECP256K1_PRIV={}", hex::encode(sk.to_bytes()));
    println!("SECP256K1_BLOB={}", hex::encode(sealed.to_bytes()));

    // ed25519 -> X25519 (Solana/Stellar/Algorand/NEAR payer)
    let seed = [0x37u8; 32];
    let signing = ed25519_dalek::SigningKey::from_bytes(&seed);
    let address = bs58::encode(signing.verifying_key().to_bytes()).into_string();
    let payer = pubkey::from_solana_address(&address).unwrap();
    let sealed = seal(body, &payer, payment_id).unwrap();
    println!("ED25519_SEED={}", hex::encode(seed));
    println!("ED25519_ADDRESS={address}");
    println!("ED25519_BLOB={}", hex::encode(sealed.to_bytes()));

    // Multi-recipient (v2): buyer + seller on the same evidence. This is the
    // vector that proves an SDK can open a bidirectional envelope from EITHER
    // side -- the property a seal/unseal round trip inside one implementation
    // cannot establish.
    let buyer = k256::SecretKey::from_slice(&[0x42u8; 32]).unwrap();
    let seller = k256::SecretKey::from_slice(&[0x55u8; 32]).unwrap();
    let sealed = x402_rs::dx402::envelope::seal_to(
        body,
        &[
            (
                x402_rs::dx402::envelope::RecipientRole::Payer,
                PayerPublicKey::Secp256k1(Box::new(buyer.public_key())),
            ),
            (
                x402_rs::dx402::envelope::RecipientRole::Seller,
                PayerPublicKey::Secp256k1(Box::new(seller.public_key())),
            ),
        ],
        payment_id,
    )
    .unwrap();
    println!("MULTI_BUYER_PRIV={}", hex::encode(buyer.to_bytes()));
    println!("MULTI_SELLER_PRIV={}", hex::encode(seller.to_bytes()));
    println!("MULTI_BLOB={}", hex::encode(sealed.to_bytes()));

    // Anchor authorization digest, so the SDKs can check their EIP-712
    // construction against this one instead of against themselves. Getting it
    // wrong does not error -- it produces a signature that simply never
    // verifies, and the anchor stays provisional with no clue why.
    let pid_b256: alloy::primitives::B256 =
        "0x1111111111111111111111111111111111111111111111111111111111111111"
            .parse()
            .unwrap();
    let ch_b256: alloy::primitives::B256 =
        "0x2222222222222222222222222222222222222222222222222222222222222222"
            .parse()
            .unwrap();
    let payee: alloy::primitives::Address = "0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8"
        .parse()
        .unwrap();
    println!(
        "ANCHOR_DIGEST_EVM={}",
        x402_rs::dx402::gate::authorization_digest(
            pid_b256,
            ch_b256,
            "s3+https://e/x",
            payee,
            8453
        )
    );
    println!(
        "ANCHOR_DIGEST_ED25519={}",
        x402_rs::dx402::gate::authorization_digest(
            pid_b256,
            ch_b256,
            "",
            alloy::primitives::Address::ZERO,
            0
        )
    );

    println!("PAYMENT_ID={}", String::from_utf8_lossy(payment_id));
    println!("BODY={}", String::from_utf8_lossy(body));
    println!("CONTENT_HASH={}", x402_rs::dx402::content_hash(body));
}

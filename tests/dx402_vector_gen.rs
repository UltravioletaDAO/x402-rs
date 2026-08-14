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

    println!("PAYMENT_ID={}", String::from_utf8_lossy(payment_id));
    println!("BODY={}", String::from_utf8_lossy(body));
    println!("CONTENT_HASH={}", x402_rs::dx402::content_hash(body));
}

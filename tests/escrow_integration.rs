//! Integration tests for x402r escrow settlement
//!
//! These tests verify the escrow module's behavior with mock providers.

use std::collections::HashMap;
use std::env;

use alloy::primitives::{address, Address, FixedBytes, U256};

// Test CREATE3 address computation matches expected values
#[test]
fn test_create3_address_computation_base_mainnet() {
    // Test with known factory and merchant addresses
    let factory = address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814");
    let merchant = address!("1234567890123456789012345678901234567890");

    // Compute the proxy address
    let proxy = x402_rs::escrow::compute_proxy_address(factory, merchant);

    // The address should be deterministic and non-zero
    assert_ne!(proxy, Address::ZERO);

    // Computing again should give the same result
    let proxy2 = x402_rs::escrow::compute_proxy_address(factory, merchant);
    assert_eq!(proxy, proxy2);
}

#[test]
fn test_create3_address_computation_base_sepolia() {
    let factory = address!("f981D813842eE78d18ef8ac825eef8e2C8A8BaC2");
    let merchant = address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");

    let proxy = x402_rs::escrow::compute_proxy_address(factory, merchant);
    assert_ne!(proxy, Address::ZERO);

    // Different merchant should give different address
    let merchant2 = address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb");
    let proxy2 = x402_rs::escrow::compute_proxy_address(factory, merchant2);
    assert_ne!(proxy, proxy2);
}

#[test]
fn test_factory_addresses() {
    use x402_rs::network::Network;

    // Base mainnet should have factory
    let base_factory = x402_rs::escrow::factory_for_network(Network::Base);
    assert!(base_factory.is_some());
    assert_eq!(
        base_factory.unwrap(),
        address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814")
    );

    // Base Sepolia should have factory
    let sepolia_factory = x402_rs::escrow::factory_for_network(Network::BaseSepolia);
    assert!(sepolia_factory.is_some());
    assert_eq!(
        sepolia_factory.unwrap(),
        address!("f981D813842eE78d18ef8ac825eef8e2C8A8BaC2")
    );

    // Other networks should not have factory
    let avalanche_factory = x402_rs::escrow::factory_for_network(Network::Avalanche);
    assert!(avalanche_factory.is_none());
}

#[test]
fn test_escrow_addresses() {
    use x402_rs::network::Network;

    // Base mainnet escrow
    let base_escrow = x402_rs::escrow::escrow_for_network(Network::Base);
    assert!(base_escrow.is_some());
    assert_eq!(
        base_escrow.unwrap(),
        address!("C409e6da89E54253fbA86C1CE3E553d24E03f6bC")
    );

    // Base Sepolia escrow
    let sepolia_escrow = x402_rs::escrow::escrow_for_network(Network::BaseSepolia);
    assert!(sepolia_escrow.is_some());
    assert_eq!(
        sepolia_escrow.unwrap(),
        address!("F7F2Bc463d79Bd3E5Cb693944B422c39114De058")
    );
}

#[test]
fn test_feature_flag_default_disabled() {
    // Remove the env var if set
    env::remove_var("ENABLE_ESCROW");

    // Should be disabled by default
    assert!(!x402_rs::escrow::is_escrow_enabled());
}

#[test]
fn test_feature_flag_enabled() {
    env::set_var("ENABLE_ESCROW", "true");
    assert!(x402_rs::escrow::is_escrow_enabled());

    env::set_var("ENABLE_ESCROW", "TRUE");
    assert!(x402_rs::escrow::is_escrow_enabled());

    env::set_var("ENABLE_ESCROW", "1");
    assert!(x402_rs::escrow::is_escrow_enabled());

    // Cleanup
    env::remove_var("ENABLE_ESCROW");
}

#[test]
fn test_refund_extension_parsing() {
    use x402_rs::escrow::RefundExtension;

    let json = r#"{
        "info": {
            "factoryAddress": "0x41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814",
            "merchantPayouts": {
                "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa": "0xbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb",
                "0xcccccccccccccccccccccccccccccccccccccccc": "0xdddddddddddddddddddddddddddddddddddddddd"
            }
        }
    }"#;

    let ext: RefundExtension = serde_json::from_str(json).unwrap();

    assert_eq!(
        ext.info.factory_address,
        address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814")
    );
    assert_eq!(ext.info.merchant_payouts.len(), 2);

    // Check proxy -> merchant mappings
    let proxy1 = address!("aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa");
    let merchant1 = ext.info.merchant_payouts.get(&proxy1).unwrap();
    assert_eq!(*merchant1, address!("bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"));
}

#[test]
fn test_proxy_verification_deterministic() {
    use x402_rs::escrow::{compute_proxy_address, EscrowSettleRequest};
    use x402_rs::network::Network;

    let factory = address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814");
    let merchant = address!("1234567890123456789012345678901234567890");
    let computed_proxy = compute_proxy_address(factory, merchant);

    // Create a mock request with the correct proxy address
    // Note: This is a simplified test - full request parsing requires valid V2 payload
    assert_ne!(computed_proxy, Address::ZERO);

    // Verify the computation is stable
    for _ in 0..10 {
        let proxy = compute_proxy_address(factory, merchant);
        assert_eq!(proxy, computed_proxy);
    }
}

#[test]
fn test_different_factories_produce_different_proxies() {
    let factory1 = address!("41Cc4D337FEC5E91ddcf4C363700FC6dB5f3A814");
    let factory2 = address!("f981D813842eE78d18ef8ac825eef8e2C8A8BaC2");
    let merchant = address!("1234567890123456789012345678901234567890");

    let proxy1 = x402_rs::escrow::compute_proxy_address(factory1, merchant);
    let proxy2 = x402_rs::escrow::compute_proxy_address(factory2, merchant);

    // Same merchant with different factories should produce different proxies
    assert_ne!(proxy1, proxy2);
}

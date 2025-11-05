#!/usr/bin/env python3
"""
Compare USDC Contracts: Facilitator Code vs Circle's Official List
"""

import json
from pathlib import Path

print("=" * 80)
print("USDC CONTRACT COMPARISON")
print("=" * 80)

# Load facilitator contracts
facilitator_json = Path("scripts/usdc_contracts_facilitator.json")
if not facilitator_json.exists():
    print("Running extract script first...")
    import subprocess
    subprocess.run(["python", "scripts/extract_usdc_contracts.py"])

with open(facilitator_json) as f:
    facilitator_data = json.load(f)

# Circle's official contracts from usdc-contracts.txt
circle_contracts = {
    # Mainnets
    "Avalanche": "0xB97EF9Ef8734C71904D8002F8b6Bc66Dd9c48a6E",
    "Base": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
    "Polygon": "0x3c499c542cEF5E3811e1192ce70d8cC03d5c3359",
    "Optimism": "0x0b2c639c533813f4aa9d7837caf62653d097ff85",
    "Sei": "0xe15fC38F6D8c56aF07bbCBe3BAf5708A2Bf42392",
    "Solana": "EPjFWdd5AufqSSqeM2qN1xzybapC8G4wEGGkZwyTDt1v",
    "XDC": "0xfA2958CB79b0491CC627c1557F441eF849Ca8eb1",
    # Testnets
    "AvalancheFuji": "0x5425890298aed601595a70AB815c96711a31Bc65",
    "BaseSepolia": "0x036cbd53842c5426634e7929541ec2318f3dcf7e",
    "PolygonAmoy": "0x41e94eb019c0762f9bfcf9fb1e58725bfb0e7582",
    "SeiTestnet": "0x4fCF1784B31630811181f670Aea7A7bEF803eaED",
    "SolanaDevnet": "4zMMC9srt5Ri5X14GAgXhaHii3GnPAEERYPJgZJDncDU",
}

# Combine mainnets and testnets from facilitator
all_facilitator = {}
all_facilitator.update(facilitator_data.get("mainnets", {}))
all_facilitator.update(facilitator_data.get("testnets", {}))

print("\n" + "=" * 80)
print("CHECKING FACILITATOR CONTRACTS AGAINST CIRCLE'S OFFICIAL LIST")
print("=" * 80)

matches = []
mismatches = []
missing_in_facilitator = []
extra_in_facilitator = []

for network, circle_address in circle_contracts.items():
    if network in all_facilitator:
        fac_address = all_facilitator[network]["address"]
        if fac_address.lower() == circle_address.lower():
            matches.append(network)
        else:
            mismatches.append({
                "network": network,
                "circle": circle_address,
                "facilitator": fac_address
            })
    else:
        missing_in_facilitator.append({
            "network": network,
            "circle_address": circle_address
        })

# Check for networks in facilitator but not in Circle's list
for network in all_facilitator:
    if network not in circle_contracts:
        extra_in_facilitator.append({
            "network": network,
            "address": all_facilitator[network]["address"]
        })

# Print results
print(f"\n[OK] MATCHES ({len(matches)} networks):")
for network in sorted(matches):
    address = all_facilitator[network]["address"]
    print(f"  {network:20s} {address}")

if mismatches:
    print(f"\n[ERROR] MISMATCHES ({len(mismatches)} networks):")
    for item in mismatches:
        print(f"  {item['network']}:")
        print(f"    Circle:      {item['circle']}")
        print(f"    Facilitator: {item['facilitator']}")

if missing_in_facilitator:
    print(f"\n[WARNING] MISSING IN FACILITATOR ({len(missing_in_facilitator)} networks):")
    for item in missing_in_facilitator:
        print(f"  {item['network']:20s} {item['circle_address']}")

if extra_in_facilitator:
    print(f"\n[INFO] EXTRA IN FACILITATOR ({len(extra_in_facilitator)} networks):")
    print("(These are in the facilitator but not in our Circle reference list)")
    for item in extra_in_facilitator:
        print(f"  {item['network']:20s} {item['address']}")

# Summary
print("\n" + "=" * 80)
print("SUMMARY")
print("=" * 80)
print(f"Total in Circle's list: {len(circle_contracts)}")
print(f"Total in Facilitator:   {len(all_facilitator)}")
print(f"Matches:                {len(matches)}")
print(f"Mismatches:             {len(mismatches)}")
print(f"Missing in Facilitator: {len(missing_in_facilitator)}")
print(f"Extra in Facilitator:   {len(extra_in_facilitator)}")

if mismatches:
    print("\n[ACTION REQUIRED] Fix mismatches in x402-rs/src/network.rs")
if missing_in_facilitator:
    print("\n[OPTIONAL] Consider adding missing networks to facilitator")

print("\n" + "=" * 80)

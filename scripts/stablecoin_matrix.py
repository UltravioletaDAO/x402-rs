#!/usr/bin/env python3
"""
Generate the stablecoin support matrix from src/network.rs

This is the GOLDEN SOURCE for stablecoin coverage.
Run this script whenever you need to know which stablecoins are supported on which networks.

Usage:
    python scripts/stablecoin_matrix.py           # Print matrix to stdout
    python scripts/stablecoin_matrix.py --json    # Output as JSON
    python scripts/stablecoin_matrix.py --md      # Output as Markdown table
"""

import re
import sys
import json
from pathlib import Path
from collections import defaultdict

def parse_network_rs():
    """Parse src/network.rs to extract stablecoin deployments."""
    network_rs = Path(__file__).parent.parent / "src" / "network.rs"

    if not network_rs.exists():
        print(f"ERROR: {network_rs} not found", file=sys.stderr)
        sys.exit(1)

    content = network_rs.read_text()

    # Pattern to match stablecoin static declarations
    # Example: static USDC_BASE: Lazy<USDCDeployment>
    pattern = r'^static ([A-Z]+)_([A-Z_]+): Lazy<'

    # Testnet suffixes to exclude
    testnet_suffixes = ['SEPOLIA', 'TESTNET', 'DEVNET', 'FUJI', 'AMOY', 'ALFAJORES']

    # Build the matrix
    matrix = defaultdict(set)  # network -> set of stablecoins
    stablecoins = set()
    networks = set()

    for line in content.split('\n'):
        match = re.match(pattern, line)
        if match:
            token = match.group(1)  # USDC, EURC, AUSD, etc.
            network = match.group(2)  # BASE, ETHEREUM, etc.

            # Skip testnets
            if any(suffix in network for suffix in testnet_suffixes):
                continue

            # Only include known stablecoins
            if token in ['USDC', 'EURC', 'AUSD', 'PYUSD', 'USDT', 'CUSD']:
                matrix[network].add(token)
                stablecoins.add(token)
                networks.add(network)

    return matrix, sorted(stablecoins), sorted(networks)

def print_matrix(matrix, stablecoins, networks):
    """Print human-readable matrix."""
    print("=" * 80)
    print("STABLECOIN SUPPORT MATRIX (Mainnets Only)")
    print("Generated from: src/network.rs")
    print("=" * 80)
    print()

    # By stablecoin
    print("BY STABLECOIN:")
    print("-" * 40)
    for token in stablecoins:
        nets = sorted([n for n, tokens in matrix.items() if token in tokens])
        print(f"  {token} ({len(nets)} networks):")
        for net in nets:
            print(f"    - {net}")
        print()

    # By network
    print("BY NETWORK:")
    print("-" * 40)
    for network in networks:
        tokens = sorted(matrix[network])
        print(f"  {network}: {', '.join(tokens)}")

    print()
    print("=" * 80)
    print(f"TOTALS: {len(stablecoins)} stablecoins across {len(networks)} networks")
    print("=" * 80)

def print_markdown(matrix, stablecoins, networks):
    """Print as Markdown table."""
    # Header
    print("| Network | " + " | ".join(stablecoins) + " |")
    print("|" + "---------|" * (len(stablecoins) + 1))

    # Rows
    for network in networks:
        row = f"| {network} |"
        for token in stablecoins:
            if token in matrix[network]:
                row += " Y |"
            else:
                row += " - |"
        print(row)

def print_json(matrix, stablecoins, networks):
    """Print as JSON."""
    output = {
        "stablecoins": stablecoins,
        "networks": networks,
        "matrix": {net: sorted(tokens) for net, tokens in matrix.items()},
        "by_stablecoin": {
            token: sorted([n for n, tokens in matrix.items() if token in tokens])
            for token in stablecoins
        }
    }
    print(json.dumps(output, indent=2))

def main():
    matrix, stablecoins, networks = parse_network_rs()

    if "--json" in sys.argv:
        print_json(matrix, stablecoins, networks)
    elif "--md" in sys.argv:
        print_markdown(matrix, stablecoins, networks)
    else:
        print_matrix(matrix, stablecoins, networks)

if __name__ == "__main__":
    main()

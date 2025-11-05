# Config Directory

This directory contains runtime configuration files for the facilitator.

## Files

### `blocklist.json` (gitignored)

Production blocklist containing addresses that should be blocked from processing payments.

**Location**: `config/blocklist.json`

**Format**:
```json
[
  {
    "account_type": "solana",
    "wallet": "SOLANA_ADDRESS_HERE",
    "reason": "reason for blocking"
  },
  {
    "account_type": "evm",
    "wallet": "0xEVM_ADDRESS_HERE",
    "reason": "reason for blocking"
  }
]
```

**Security**: This file is gitignored to prevent accidental commits containing sensitive information about blocked addresses.

### `blocklist.json.example`

Template file showing the structure of the blocklist. Copy this to `blocklist.json` and modify as needed.

## Usage

1. Copy `blocklist.json.example` to `blocklist.json`
2. Edit `blocklist.json` with actual addresses to block
3. Restart the facilitator to load changes

**Note**: The blocklist is loaded at startup. Changes require a facilitator restart.

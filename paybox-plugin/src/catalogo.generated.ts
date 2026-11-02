// GENERATED: do not edit by hand. src/mcp.rs writes it, in process and without the network, from
// the facilitator's own tools/list: the tools of class `lectura` (_meta "uvd/clase"), exactly as
// POST /mcp lists them. Regenerate with X402_WRITE_PAYBOX_PLUGIN=1 and the test
// mcp::tests::the_paybox_plugin_relays_exactly_the_reads_of_tools_list (README.md, "Regenerar");
// without that variable the test compares this file byte for byte. A string of 0x + 64 hex is
// written with its x escaped (\u0078): the same string, which the pre-commit hook does not take
// for a private key.
export const CATALOGO: unknown = {
  "esquema": 1,
  "ruta": "/mcp",
  "tools": [
    {
      "name": "x402_supported",
      "description": "List every payment scheme and network this facilitator settles on, in both the x402 v1 (\"base\") and CAIP-2 (\"eip155:8453\") spellings, plus the protocol extensions it serves. Read this before building a payment: it is the only authoritative answer to \"can you settle X on Y\". Takes no arguments. Equivalent to GET /supported.",
      "inputSchema": {
        "type": "object",
        "properties": {},
        "additionalProperties": false
      },
      "outputSchema": {
        "type": "object",
        "description": "The body of GET /supported, unchanged: every (x402Version, scheme, network) this facilitator settles, the protocol extensions it serves, the fee payer per network where it differs, and what its facilitator receipts cover.",
        "required": [
          "kinds"
        ],
        "properties": {
          "kinds": {
            "type": "array",
            "description": "One entry per settleable (x402Version, scheme, network). Every chain appears twice, under its x402 v1 name (\"base\") and under its CAIP-2 id (\"eip155:8453\"); `networkAliases` lists both on each entry.",
            "items": {
              "type": "object",
              "required": [
                "x402Version",
                "scheme",
                "network"
              ],
              "properties": {
                "x402Version": {
                  "type": "integer",
                  "minimum": 1,
                  "description": "The x402 protocol version this entry is for."
                },
                "scheme": {
                  "type": "string",
                  "description": "Payment scheme: exact, upto, escrow or commerce."
                },
                "network": {
                  "type": "string",
                  "description": "The network, as the x402 version of this entry spells it."
                },
                "networkAliases": {
                  "type": "array",
                  "items": {
                    "type": "string"
                  },
                  "description": "Every identifier of the same chain, this one included."
                },
                "extra": {
                  "type": "object",
                  "description": "Network-specific settlement details.",
                  "properties": {
                    "feePayer": {
                      "type": "string",
                      "description": "The account that pays the network fee."
                    },
                    "tokens": {
                      "type": "array",
                      "items": {
                        "type": "object",
                        "required": [
                          "token",
                          "address",
                          "decimals"
                        ],
                        "properties": {
                          "token": {
                            "type": "string"
                          },
                          "address": {
                            "type": "string"
                          },
                          "decimals": {
                            "type": "integer",
                            "minimum": 0
                          }
                        }
                      }
                    },
                    "escrowAddress": {
                      "type": "string"
                    },
                    "operatorAddress": {
                      "type": "string"
                    },
                    "tokenCollector": {
                      "type": "string"
                    }
                  }
                }
              }
            }
          },
          "extensions": {
            "type": "array",
            "items": {
              "type": "string"
            },
            "description": "Protocol extensions this deployment serves."
          },
          "signers": {
            "type": "object",
            "additionalProperties": {
              "type": "array",
              "items": {
                "type": "string"
              }
            },
            "description": "Fee payer accounts by network, where they differ per network."
          },
          "facilitatorReceipts": {
            "type": "object",
            "description": "The facilitator receipt capability; see GET /receipts."
          }
        }
      },
      "annotations": {
        "title": "Supported networks and schemes",
        "readOnlyHint": true,
        "idempotentHint": true,
        "openWorldHint": false
      },
      "_meta": {
        "uvd/clase": "lectura"
      }
    },
    {
      "name": "x402_networks",
      "description": "How to present every network this facilitator settles on: display name, chain family, chain id, testnet flag, explorer URL templates for a transaction and an address, icon, schemes, and each token's symbol, address, decimals and EIP-712 domain. One row per chain x402_supported names, never more or fewer: use x402_supported to learn whether a payment can be settled, and this to name the chain and link its transaction. Takes no arguments. Moves no money and signs nothing. Equivalent to GET /networks.json.",
      "inputSchema": {
        "type": "object",
        "properties": {},
        "additionalProperties": false
      },
      "outputSchema": {
        "type": "object",
        "description": "The body of GET /networks.json, unchanged: one row per chain x402_supported names -- never more, never fewer -- with how to present it.",
        "required": [
          "networks"
        ],
        "properties": {
          "networks": {
            "type": "array",
            "items": {
              "type": "object",
              "required": [
                "id",
                "caip2",
                "family",
                "testnet",
                "displayName",
                "schemes",
                "tokens"
              ],
              "properties": {
                "id": {
                  "type": "string",
                  "description": "The x402 v1 name (\"base\"). A chain with no v1 name (native Hedera) repeats its CAIP-2 id here."
                },
                "caip2": {
                  "type": "string",
                  "description": "The CAIP-2 id (\"eip155:8453\")."
                },
                "family": {
                  "type": "string",
                  "description": "evm, svm, near, stellar, algorand, sui, xrpl or hedera."
                },
                "chainId": {
                  "type": [
                    "integer",
                    "null"
                  ],
                  "minimum": 0,
                  "description": "The EVM chain id; null outside EVM."
                },
                "testnet": {
                  "type": "boolean"
                },
                "displayName": {
                  "type": "string"
                },
                "explorer": {
                  "type": [
                    "object",
                    "null"
                  ],
                  "description": "Explorer URL templates; null when none is known.",
                  "required": [
                    "base",
                    "tx",
                    "address"
                  ],
                  "properties": {
                    "base": {
                      "type": "string"
                    },
                    "tx": {
                      "type": "string",
                      "description": "Carries a {tx} placeholder for the transaction id."
                    },
                    "address": {
                      "type": "string",
                      "description": "Carries an {address} placeholder."
                    }
                  }
                },
                "icon": {
                  "type": [
                    "string",
                    "null"
                  ]
                },
                "schemes": {
                  "type": "array",
                  "items": {
                    "type": "string"
                  }
                },
                "tokens": {
                  "type": "array",
                  "items": {
                    "type": "object",
                    "required": [
                      "symbol",
                      "address",
                      "decimals"
                    ],
                    "properties": {
                      "symbol": {
                        "type": "string"
                      },
                      "address": {
                        "type": "string"
                      },
                      "decimals": {
                        "type": "integer",
                        "minimum": 0
                      },
                      "eip712": {
                        "type": [
                          "object",
                          "null"
                        ],
                        "description": "The EIP-712 domain /verify resolves for this token; null outside EVM or outside the static table.",
                        "properties": {
                          "name": {
                            "type": "string"
                          },
                          "version": {
                            "type": "string"
                          }
                        }
                      },
                      "usdPegged": {
                        "type": [
                          "boolean",
                          "null"
                        ]
                      },
                      "icon": {
                        "type": [
                          "string",
                          "null"
                        ]
                      }
                    }
                  }
                }
              }
            }
          }
        }
      },
      "annotations": {
        "title": "Network directory: names, explorers, tokens",
        "readOnlyHint": true,
        "idempotentHint": true,
        "openWorldHint": false
      },
      "_meta": {
        "uvd/clase": "lectura"
      }
    },
    {
      "name": "x402_chain_health",
      "description": "Whether this facilitator can settle right now, chain by chain: RPC reachability and how many settles each signer's gas still covers, graded ok, degraded or down with a bounded reason. Use it to tell a payment that did not settle because the chain or the facilitator's signer is down from one refused on its merits. Optional `network` scopes the answer to one chain, in either spelling (\"base\" or \"eip155:8453\"). The reading is cached for ttlSecs and says when it was measured; a chain that reads down comes back as this result, not as an error. Moves no money and signs nothing. Equivalent to GET /health/ready.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "network": {
            "type": "string",
            "description": "Optional. One chain, as its x402 v1 name (\"base\") or its CAIP-2 id (\"eip155:8453\"). Omit it for every configured chain."
          }
        },
        "additionalProperties": false
      },
      "outputSchema": {
        "type": "object",
        "description": "The body of GET /health/ready, unchanged: whether this facilitator can settle, chain by chain, as measured at checkedAtUnix and cached for ttlSecs. No RPC URL, address or balance is ever in it.",
        "required": [
          "status",
          "networks"
        ],
        "properties": {
          "status": {
            "type": "string",
            "enum": [
              "ok",
              "degraded",
              "down"
            ],
            "description": "The worst reading in scope. down: a mainnet in scope cannot settle. Unscoped, an old sanctions list makes it degraded, never down."
          },
          "checkedAtUnix": {
            "type": "integer",
            "minimum": 0
          },
          "ageSecs": {
            "type": "integer",
            "minimum": 0
          },
          "ttlSecs": {
            "type": "integer",
            "minimum": 0
          },
          "probeTimeoutMs": {
            "type": "integer",
            "minimum": 0
          },
          "thresholds": {
            "type": "object"
          },
          "summary": {
            "type": "object",
            "properties": {
              "ok": {
                "type": "integer",
                "minimum": 0
              },
              "degraded": {
                "type": "integer",
                "minimum": 0
              },
              "down": {
                "type": "integer",
                "minimum": 0
              }
            }
          },
          "networks": {
            "type": "array",
            "items": {
              "type": "object",
              "required": [
                "network",
                "caip2",
                "mainnet",
                "status",
                "rpc",
                "signers"
              ],
              "properties": {
                "network": {
                  "type": "string"
                },
                "caip2": {
                  "type": "string"
                },
                "mainnet": {
                  "type": "boolean"
                },
                "status": {
                  "type": "string",
                  "enum": [
                    "ok",
                    "degraded",
                    "down"
                  ]
                },
                "reason": {
                  "type": "string",
                  "description": "A bounded token, present unless status is ok: rpc_unreachable, rpc_timeout, rpc_chain_id_mismatch, signer_gas_critical, signer_gas_low, and on native Hedera also signer_key_mismatch and store_unavailable."
                },
                "rpc": {
                  "type": "string",
                  "description": "ok, unreachable, timeout or wrong_chain."
                },
                "warnSettles": {
                  "type": "integer",
                  "minimum": 0
                },
                "signers": {
                  "type": "array",
                  "items": {
                    "type": "object",
                    "required": [
                      "index",
                      "status",
                      "gasOk"
                    ],
                    "properties": {
                      "index": {
                        "type": "integer",
                        "minimum": 0
                      },
                      "status": {
                        "type": "string",
                        "enum": [
                          "ok",
                          "degraded",
                          "down"
                        ]
                      },
                      "gasOk": {
                        "type": "boolean"
                      },
                      "settlesRemaining": {
                        "type": [
                          "integer",
                          "null"
                        ],
                        "minimum": 0,
                        "description": "Settles the signer's gas still admits; null where the chain prices gas at zero."
                      }
                    }
                  }
                }
              }
            }
          },
          "unchecked": {
            "type": "array",
            "items": {
              "type": "string"
            },
            "description": "Configured networks whose family is not probed: listed here rather than folded into a green answer."
          },
          "sanctionsList": {
            "type": "object"
          }
        }
      },
      "annotations": {
        "title": "Settlement health per chain",
        "readOnlyHint": true,
        "idempotentHint": true,
        "openWorldHint": true
      },
      "_meta": {
        "uvd/clase": "lectura"
      }
    },
    {
      "name": "x402_escrow_onchain_state",
      "description": "Read what ONE x402r escrow payment holds on-chain, from its AuthCaptureEscrow contract: whether the funds were collected, how much is still capturable and how much is refundable. A view call on the contract -- no transaction and no gas -- so it answers what the chain holds for this payment, not what any marketplace recorded about a task. The arguments identify the payment (the paymentInfo, payer, CAIP-2 network and escrow contracts it was made with); nothing is signed. Moves no money. Equivalent to POST /escrow/state.",
      "inputSchema": {
        "type": "object",
        "description": "Identical to the JSON body of POST /escrow/state. These fields IDENTIFY one escrow payment -- the paymentInfo it was authorized with, its payer, its chain and its escrow contracts. Nothing in them is signed and nothing is spent.",
        "properties": {
          "paymentInfo": {
            "type": "object",
            "description": "The AuthCaptureEscrow PaymentInfo the payment was made with, as the escrow payload carried it (the payer goes in `payer`).",
            "properties": {
              "operator": {
                "type": "string",
                "description": "PaymentOperator contract, 0x + 40 hex."
              },
              "receiver": {
                "type": "string",
                "description": "Who receives the payment, minus fees, 0x + 40 hex."
              },
              "token": {
                "type": "string",
                "description": "Token contract, 0x + 40 hex."
              },
              "maxAmount": {
                "type": "string",
                "description": "Most the escrow may hold, in token base units, as a decimal STRING."
              },
              "preApprovalExpiry": {
                "type": "integer",
                "minimum": 0,
                "description": "uint48 unix seconds, as a JSON number."
              },
              "authorizationExpiry": {
                "type": "integer",
                "minimum": 0,
                "description": "uint48 unix seconds, as a JSON number."
              },
              "refundExpiry": {
                "type": "integer",
                "minimum": 0,
                "description": "uint48 unix seconds, as a JSON number."
              },
              "minFeeBps": {
                "type": "integer",
                "minimum": 0,
                "maximum": 65535
              },
              "maxFeeBps": {
                "type": "integer",
                "minimum": 0,
                "maximum": 65535
              },
              "feeReceiver": {
                "type": "string",
                "description": "Who receives the fee, 0x + 40 hex."
              },
              "salt": {
                "type": "string",
                "description": "The payment's 32-byte salt, 0x + 64 hex."
              }
            },
            "required": [
              "operator",
              "receiver",
              "token",
              "maxAmount",
              "preApprovalExpiry",
              "authorizationExpiry",
              "refundExpiry",
              "minFeeBps",
              "maxFeeBps",
              "feeReceiver",
              "salt"
            ],
            "additionalProperties": true
          },
          "payer": {
            "type": "string",
            "description": "The payer, 0x + 40 hex."
          },
          "network": {
            "type": "string",
            "description": "The chain as a CAIP-2 id (\"eip155:8453\"). CAIP-2 ONLY: this route refuses the bare x402 v1 name (\"base\")."
          },
          "extra": {
            "type": "object",
            "description": "The escrow contracts, as the payment requirements' `extra` named them.",
            "properties": {
              "escrowAddress": {
                "type": "string",
                "description": "AuthCaptureEscrow contract, 0x + 40 hex."
              },
              "operatorAddress": {
                "type": "string",
                "description": "PaymentOperator contract, 0x + 40 hex."
              },
              "tokenCollector": {
                "type": "string",
                "description": "ERC3009TokenCollector contract, 0x + 40 hex."
              }
            },
            "required": [
              "escrowAddress",
              "operatorAddress",
              "tokenCollector"
            ],
            "additionalProperties": true
          }
        },
        "required": [
          "paymentInfo",
          "payer",
          "network",
          "extra"
        ],
        "additionalProperties": true,
        "examples": [
          {
            "paymentInfo": {
              "operator": "0x1111111111111111111111111111111111111111",
              "receiver": "0x2222222222222222222222222222222222222222",
              "token": "0x833589fcd6edb6e08f4c7c32d4f71b54bda02913",
              "maxAmount": "1000000",
              "preApprovalExpiry": 281474976710655,
              "authorizationExpiry": 281474976710655,
              "refundExpiry": 281474976710655,
              "minFeeBps": 0,
              "maxFeeBps": 100,
              "feeReceiver": "0x3333333333333333333333333333333333333333",
              "salt": "0\u00785a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a5a"
            },
            "payer": "0x4444444444444444444444444444444444444444",
            "network": "eip155:8453",
            "extra": {
              "escrowAddress": "0x5555555555555555555555555555555555555555",
              "operatorAddress": "0x1111111111111111111111111111111111111111",
              "tokenCollector": "0x6666666666666666666666666666666666666666"
            }
          }
        ]
      },
      "outputSchema": {
        "type": "object",
        "description": "The body of POST /escrow/state, unchanged: the escrow contract's own view of one payment, read with a view call.",
        "required": [
          "hasCollectedPayment",
          "capturableAmount",
          "refundableAmount",
          "paymentInfoHash",
          "network"
        ],
        "properties": {
          "hasCollectedPayment": {
            "type": "boolean",
            "description": "Whether the escrow has collected the payer's funds for this payment."
          },
          "capturableAmount": {
            "type": "string",
            "pattern": "^[0-9]+$",
            "description": "Still available for capture, in token base units, as a decimal string."
          },
          "refundableAmount": {
            "type": "string",
            "pattern": "^[0-9]+$",
            "description": "Available for refund, in token base units, as a decimal string."
          },
          "paymentInfoHash": {
            "type": "string",
            "description": "The hash the escrow keys this payment by."
          },
          "network": {
            "type": "string",
            "description": "The CAIP-2 id of the chain the escrow lives on."
          }
        }
      },
      "annotations": {
        "title": "On-chain escrow state (read-only)",
        "readOnlyHint": true,
        "idempotentHint": true,
        "openWorldHint": true
      },
      "_meta": {
        "uvd/clase": "lectura"
      }
    },
    {
      "name": "x402_durable_evidence",
      "description": "Look up the DX402 durable evidence anchored for a settled payment: where the sealed copy of the paid response lives, the hash of what was delivered, its retention, whether the payee proved the anchor (verified/signed) and the facilitator's signed receipt. `document: \"receipt\"` returns the signed receipt alone, with its EIP-712 domain, for offline verification. 404 means nothing was anchored for that paymentId and 410 that it was and expired -- different answers in a dispute. Served only where x402_supported lists `durable-evidence` in `extensions`; elsewhere the call answers the generic 404. Moves no money, signs nothing and never returns the sealed bytes. Equivalent to GET /dx402/evidence/{paymentId} or GET /dx402/receipt/{paymentId}.",
      "inputSchema": {
        "type": "object",
        "properties": {
          "paymentId": {
            "type": "string",
            "description": "The DX402 paymentId, keccak256(caip2Network || txHash), 0x + 64 hex: the id the seller's X-Durable-Evidence header or its anchor reported. Case and the 0x prefix do not matter."
          },
          "document": {
            "type": "string",
            "enum": [
              "evidence",
              "receipt"
            ],
            "default": "evidence",
            "description": "evidence (the default): pointer, contentHash, retention, verified/signed and the receipt's signature. receipt: the signed EIP-712 receipt alone, with its domain, for offline verification."
          }
        },
        "required": [
          "paymentId"
        ],
        "additionalProperties": false
      },
      "outputSchema": {
        "type": "object",
        "description": "The body of GET /dx402/evidence/{paymentId} or, with document=receipt, of GET /dx402/receipt/{paymentId}, unchanged.",
        "anyOf": [
          {
            "title": "evidence",
            "required": [
              "paymentId",
              "pointer",
              "contentHash",
              "receipt",
              "receiptSigner",
              "verified",
              "signed"
            ],
            "properties": {
              "receipt": {
                "type": "string"
              }
            }
          },
          {
            "title": "receipt",
            "required": [
              "receipt",
              "signature",
              "signer",
              "domain"
            ],
            "properties": {
              "receipt": {
                "type": "object"
              }
            }
          }
        ],
        "properties": {
          "paymentId": {
            "type": "string"
          },
          "pointer": {
            "type": "string",
            "description": "Where the sealed copy lives. Ciphertext only, sealed to the payer's own key; GET /dx402/blob/{paymentId} serves it."
          },
          "backend": {
            "type": "string"
          },
          "contentHash": {
            "type": "string",
            "description": "Hash of the PLAINTEXT that was delivered, so it proves what the evidence decrypts to."
          },
          "keyAlg": {
            "type": "string"
          },
          "mode": {
            "type": "string"
          },
          "retention": {
            "type": "string"
          },
          "anchoredAt": {
            "type": "integer",
            "minimum": 0
          },
          "retentionUntil": {
            "type": "integer",
            "minimum": 0
          },
          "receipt": {
            "description": "evidence: the facilitator's signature over the receipt. receipt: the signed receipt itself."
          },
          "receiptSigner": {
            "type": "string"
          },
          "verified": {
            "type": "boolean",
            "description": "The payee proved the anchor is theirs. false is provisional: a claim anyone could have made."
          },
          "signed": {
            "type": "boolean",
            "description": "The claimant committed to an identity, not yet confirmed by a chain to be the payee."
          },
          "signature": {
            "type": "string"
          },
          "signer": {
            "type": "string"
          },
          "domain": {
            "type": "object",
            "required": [
              "name",
              "version",
              "chainId"
            ],
            "properties": {
              "name": {
                "type": "string"
              },
              "version": {
                "type": "string"
              },
              "chainId": {
                "type": "integer",
                "minimum": 0
              }
            }
          }
        }
      },
      "annotations": {
        "title": "DX402 durable evidence of a payment",
        "readOnlyHint": true,
        "idempotentHint": true,
        "openWorldHint": false
      },
      "_meta": {
        "uvd/clase": "lectura"
      }
    }
  ]
};

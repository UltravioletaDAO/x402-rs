#!/usr/bin/env python3
"""
ERC-8004 Feedback Integration Test

Este script demuestra cómo funciona la integración ERC-8004 con el facilitador x402.

FLUJO COMPLETO:
===============

1. SETTLEMENT CON EXTENSION 8004-reputation
   - Cliente hace un pago x402 normal
   - En PaymentRequirements.extra incluye {"8004-reputation": {"include_proof": true}}
   - El facilitador ejecuta el pago y retorna un ProofOfPayment

2. PROOF OF PAYMENT
   - El ProofOfPayment es una prueba criptográfica de que el pago ocurrió
   - Contiene: transaction_hash, block_number, payer, payee, amount, token, timestamp
   - Se usa para demostrar que pagaste antes de dar feedback

3. SUBMIT FEEDBACK
   - Con el ProofOfPayment, puedes enviar feedback al ReputationRegistry
   - POST /feedback con: agent address, score (1-5), comment, y el proof
   - El facilitador llama al contrato submitFeedback() en Ethereum

EJEMPLO DE USO:
===============

# Paso 1: Hacer un pago con la extensión 8004-reputation
payment_requirements = {
    "scheme": "exact",
    "network": "ethereum-mainnet",
    "maxAmountRequired": "1000000",  # 1 USDC
    "asset": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",  # USDC en Ethereum
    "payTo": "0xAgentAddress",
    "extra": {
        "8004-reputation": {
            "include_proof": true
        }
    }
}

# Paso 2: El settlement response incluye proof_of_payment
settle_response = {
    "success": true,
    "transaction": "0x...",
    "proof_of_payment": {
        "transaction_hash": "0x...",
        "block_number": 12345678,
        "network": "ethereum-mainnet",
        "payer": "0xPayer...",
        "payee": "0xAgent...",
        "amount": "1000000",
        "token": "0xUSDC...",
        "timestamp": 1706500000,
        "payment_hash": "0x..."
    }
}

# Paso 3: Enviar feedback usando el proof
feedback_request = {
    "x402_version": 1,
    "network": "ethereum-mainnet",
    "feedback": {
        "agent": "0xAgentAddress",
        "score": 5,
        "comment": "Excellent AI agent!",
        "proof": settle_response["proof_of_payment"]
    }
}
"""

import requests
import json
import sys
from datetime import datetime

# Configuracion
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"

def test_feedback_endpoint_info():
    """Test 1: Verificar que el endpoint /feedback existe y retorna la info correcta"""
    print("\n" + "="*60)
    print("TEST 1: GET /feedback - Informacion del endpoint")
    print("="*60)

    response = requests.get(f"{FACILITATOR_URL}/feedback")
    print(f"Status: {response.status_code}")

    if response.status_code == 200:
        data = response.json()
        print(f"\nEndpoint: {data.get('endpoint')}")
        print(f"Extension: {data.get('extension')}")
        print(f"\nContratos:")
        contracts = data.get('contracts', {})
        print(f"  ReputationRegistry: {contracts.get('reputationRegistry')}")
        print(f"  Redes soportadas: {contracts.get('supportedNetworks')}")

        # Verificar que solo Ethereum mainnet esta soportado
        supported = contracts.get('supportedNetworks', [])
        if supported == ["ethereum-mainnet"]:
            print("\n[OK] Solo Ethereum Mainnet esta soportado (correcto)")
        else:
            print(f"\n[WARN] Redes soportadas inesperadas: {supported}")

        return True
    else:
        print(f"[ERROR] Response: {response.text}")
        return False


def test_feedback_validation():
    """Test 2: Verificar validacion del endpoint /feedback"""
    print("\n" + "="*60)
    print("TEST 2: POST /feedback - Validacion de errores")
    print("="*60)

    # Test 2a: Request sin body
    print("\n2a. Request vacio:")
    response = requests.post(f"{FACILITATOR_URL}/feedback", json={})
    print(f"  Status: {response.status_code}")
    if response.status_code == 400:
        print("  [OK] Rechazado correctamente (400 Bad Request)")

    # Test 2b: Network no soportada
    print("\n2b. Network no soportada (base-mainnet):")
    response = requests.post(f"{FACILITATOR_URL}/feedback", json={
        "x402_version": 1,
        "network": "base-mainnet",  # No soportada para ERC-8004
        "feedback": {
            "agent": "0x1234567890123456789012345678901234567890",
            "score": 5,
            "proof": {
                "transaction_hash": "0x" + "a"*64,
                "block_number": 12345678,
                "network": "base-mainnet",
                "payer": "0x" + "1"*40,
                "payee": "0x" + "2"*40,
                "amount": "1000000",
                "token": "0x" + "3"*40,
                "timestamp": 1706500000,
                "payment_hash": "0x" + "b"*64
            }
        }
    })
    print(f"  Status: {response.status_code}")
    data = response.json()
    if "not supported" in data.get("error", "").lower() or response.status_code == 400:
        print("  [OK] Rechazado correctamente - red no soportada")
    print(f"  Response: {json.dumps(data, indent=2)}")

    return True


def test_feedback_with_fake_proof():
    """Test 3: Intentar enviar feedback con proof falso (debe fallar en on-chain)"""
    print("\n" + "="*60)
    print("TEST 3: POST /feedback - Proof falso en Ethereum Mainnet")
    print("="*60)
    print("\nNOTA: Este test enviara una transaccion real a Ethereum.")
    print("El proof es falso, asi que el contrato deberia rechazarlo.")
    print("Esto costara gas al facilitador.\n")

    # Proof falso pero con formato correcto
    fake_proof = {
        "transaction_hash": "0x" + "a"*64,
        "block_number": 12345678,
        "network": "ethereum-mainnet",
        "payer": "0x1234567890123456789012345678901234567890",
        "payee": "0x2345678901234567890123456789012345678901",
        "amount": "1000000",
        "token": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",  # USDC real
        "timestamp": 1706500000,
        "payment_hash": "0x" + "b"*64
    }

    feedback_request = {
        "x402_version": 1,
        "network": "ethereum-mainnet",
        "feedback": {
            "agent": "0x8004A169FB4a3325136EB29fA0ceB6D2e539a432",  # IdentityRegistry como "agent"
            "score": 5,
            "comment": "Test feedback from x402-rs facilitator",
            "proof": fake_proof
        }
    }

    print("Request:")
    print(json.dumps(feedback_request, indent=2))

    # DESCOMENTAR para ejecutar (costara gas!)
    # response = requests.post(f"{FACILITATOR_URL}/feedback", json=feedback_request)
    # print(f"\nStatus: {response.status_code}")
    # print(f"Response: {json.dumps(response.json(), indent=2)}")

    print("\n[SKIP] Test comentado para evitar gastar gas con proof falso")
    print("Descomenta las lineas para ejecutar el test real")

    return True


def demo_real_flow():
    """Demostrar el flujo real completo (sin ejecutar)"""
    print("\n" + "="*60)
    print("DEMO: Flujo completo ERC-8004")
    print("="*60)

    print("""
PASO 1: HACER UN PAGO x402 CON EXTENSION 8004-reputation
=========================================================

El cliente incluye la extension en PaymentRequirements:

    payment_requirements = {
        "scheme": "exact",
        "network": "ethereum-mainnet",
        "maxAmountRequired": "1000000",
        "asset": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
        "payTo": "0xAgentWalletAddress",
        "extra": {
            "8004-reputation": {
                "include_proof": true    <-- Esto activa el proof
            }
        }
    }

PASO 2: EL FACILITADOR RETORNA ProofOfPayment
=============================================

El SettleResponse incluye el proof:

    {
        "success": true,
        "transaction": "0xabc123...",
        "network": "ethereum-mainnet",
        "proof_of_payment": {           <-- Esto es nuevo!
            "transaction_hash": "0xabc123...",
            "block_number": 19500000,
            "network": "ethereum-mainnet",
            "payer": "0xClientWallet...",
            "payee": "0xAgentWallet...",
            "amount": "1000000",
            "token": "0xA0b86991c6218b36c1d19D4a2e9Eb0cE3606eB48",
            "timestamp": 1706500000,
            "payment_hash": "0xdef456..."
        }
    }

PASO 3: ENVIAR FEEDBACK CON EL PROOF
====================================

El cliente puede ahora enviar feedback:

    POST /feedback
    {
        "x402_version": 1,
        "network": "ethereum-mainnet",
        "feedback": {
            "agent": "0xAgentWalletAddress",
            "score": 5,
            "comment": "Excellent service!",
            "proof": <proof_of_payment del paso 2>
        }
    }

El facilitador:
1. Verifica que el proof es valido
2. Llama al ReputationRegistry.submitFeedback()
3. Retorna el tx hash de la transaccion de feedback

RESULTADO FINAL
===============

El agente ahora tiene una puntuacion de reputacion on-chain en:
ReputationRegistry: 0x8004BAa17C55a88189AE136b182e5fdA19dE9b63

Cualquiera puede leer la reputacion del agente llamando:
    registry.getReputation(agentAddress) -> (score, feedbackCount)
""")


def main():
    print("\n" + "#"*60)
    print("# ERC-8004 FEEDBACK INTEGRATION TEST")
    print("# Facilitator:", FACILITATOR_URL)
    print("# Fecha:", datetime.now().isoformat())
    print("#"*60)

    # Ejecutar tests
    test_feedback_endpoint_info()
    test_feedback_validation()
    test_feedback_with_fake_proof()
    demo_real_flow()

    print("\n" + "="*60)
    print("RESUMEN")
    print("="*60)
    print("""
Para usar ERC-8004 feedback en produccion:

1. El CLIENTE debe incluir en su PaymentRequirements:
   "extra": {"8004-reputation": {"include_proof": true}}

2. El FACILITADOR retornara proof_of_payment en el SettleResponse

3. El CLIENTE puede POST /feedback con ese proof para dar feedback

Contratos en Ethereum Mainnet:
- IdentityRegistry:   0x8004A169FB4a3325136EB29fA0ceB6D2e539a432
- ReputationRegistry: 0x8004BAa17C55a88189AE136b182e5fdA19dE9b63
""")


if __name__ == "__main__":
    main()

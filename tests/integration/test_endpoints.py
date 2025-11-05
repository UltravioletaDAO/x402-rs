#!/usr/bin/env python3
"""
Test All Karmacadabra Endpoints
Tests facilitator + all agents after stack deployment
"""

import requests
import json
import sys
from typing import Dict, Tuple

# Colors for terminal output
GREEN = '\033[92m'
RED = '\033[91m'
YELLOW = '\033[93m'
BLUE = '\033[94m'
CYAN = '\033[96m'
RESET = '\033[0m'

# Production endpoints
FACILITATOR_URL = "https://facilitator.ultravioletadao.xyz"
BASE_DOMAIN = "karmacadabra.ultravioletadao.xyz"

AGENTS = {
    "Validator": f"https://validator.{BASE_DOMAIN}",
    "Karma-Hello": f"https://karma-hello.{BASE_DOMAIN}",
    "Abracadabra": f"https://abracadabra.{BASE_DOMAIN}",
    "Skill-Extractor": f"https://skill-extractor.{BASE_DOMAIN}",
    "Voice-Extractor": f"https://voice-extractor.{BASE_DOMAIN}",
}


def test_endpoint(name: str, url: str, timeout: int = 5) -> Tuple[bool, str]:
    """Test a single endpoint and return (success, message)"""
    try:
        response = requests.get(url, timeout=timeout)
        if response.status_code == 200:
            try:
                data = response.json()
                return True, json.dumps(data, indent=2)
            except json.JSONDecodeError:
                return True, response.text[:200]
        else:
            return False, f"HTTP {response.status_code}"
    except requests.exceptions.Timeout:
        return False, "Timeout (no response)"
    except requests.exceptions.ConnectionError:
        return False, "Connection refused"
    except Exception as e:
        return False, str(e)


def print_section(title: str):
    """Print a section header"""
    print(f"\n{BLUE}{'='*80}{RESET}")
    print(f"{BLUE}{title}{RESET}")
    print(f"{BLUE}{'='*80}{RESET}\n")


def print_test_result(name: str, endpoint: str, success: bool, message: str, show_data: bool = False):
    """Print test result with formatting"""
    status = f"{GREEN}[OK]{RESET}" if success else f"{RED}[FAIL]{RESET}"
    print(f"{status} {name}")
    print(f"  Endpoint: {CYAN}{endpoint}{RESET}")

    if show_data and success:
        # Pretty print JSON data
        lines = message.split('\n')
        for line in lines[:10]:  # Show first 10 lines
            print(f"  {line}")
        if len(lines) > 10:
            print(f"  ... ({len(lines) - 10} more lines)")
    elif not success:
        print(f"  Error: {RED}{message}{RESET}")
    print()


def test_facilitator():
    """Test all facilitator endpoints"""
    print_section("FACILITATOR ENDPOINTS")

    results = {}

    # Test 1: Health
    print(f"{YELLOW}1. Health Check{RESET}")
    print("-" * 80)
    endpoint = f"{FACILITATOR_URL}/health"
    success, message = test_endpoint("Facilitator Health", endpoint)
    results['health'] = success
    print_test_result("Facilitator /health", endpoint, success, message, show_data=True)

    # Test 2: Supported Methods
    print(f"{YELLOW}2. Supported Payment Methods{RESET}")
    print("-" * 80)
    endpoint = f"{FACILITATOR_URL}/supported"
    success, message = test_endpoint("Supported Methods", endpoint)
    results['supported'] = success
    print_test_result("Facilitator /supported", endpoint, success, message, show_data=True)

    # Test 3: Verify Endpoint (requires payload - just test if it responds)
    print(f"{YELLOW}3. Verify Endpoint (Empty Request){RESET}")
    print("-" * 80)
    endpoint = f"{FACILITATOR_URL}/verify"
    try:
        # Send empty POST - should fail with 400 but proves endpoint exists
        response = requests.post(endpoint, json={}, timeout=5)
        # Accept 400 (bad request) as proof endpoint is live
        if response.status_code in [200, 400, 422]:
            success = True
            message = f"HTTP {response.status_code} - Endpoint responding"
        else:
            success = False
            message = f"HTTP {response.status_code}"
        results['verify'] = success
        print_test_result("Facilitator /verify", endpoint, success, message)
    except Exception as e:
        results['verify'] = False
        print_test_result("Facilitator /verify", endpoint, False, str(e))

    return results


def test_agent(name: str, url: str) -> Dict[str, bool]:
    """Test all endpoints for a single agent"""
    results = {}

    # Test 1: Health
    endpoint = f"{url}/health"
    success, message = test_endpoint(f"{name} Health", endpoint)
    results['health'] = success
    print_test_result(f"{name} /health", endpoint, success, message, show_data=True)

    # Test 2: AgentCard
    endpoint = f"{url}/.well-known/agent-card"
    success, message = test_endpoint(f"{name} AgentCard", endpoint)
    results['agent_card'] = success

    if success:
        try:
            card = json.loads(message)
            print(f"{GREEN}[OK]{RESET} {name} /.well-known/agent-card")
            print(f"  Endpoint: {CYAN}{endpoint}{RESET}")
            print(f"  Domain: {card.get('domain', 'N/A')}")
            print(f"  Agent ID: {card.get('agentId', card.get('agent_id', 'N/A'))}")

            skills = card.get('skills', [])
            print(f"  Skills: {len(skills)} available")

            for skill in skills[:3]:  # Show first 3 skills
                skill_name = skill.get('name', skill.get('skillId', 'Unknown'))

                # Handle different pricing formats
                pricing = skill.get('pricing', skill.get('price', {}))
                if isinstance(pricing, dict):
                    if 'amount' in pricing:
                        price = pricing['amount']
                    elif 'base_price' in pricing:
                        price = pricing['base_price']
                    # Check for tiered pricing (basic, standard, complete, enterprise)
                    elif 'basic' in pricing or 'standard' in pricing:
                        # Format tiered pricing
                        tiers = []
                        for tier in ['basic', 'standard', 'complete', 'enterprise']:
                            if tier in pricing:
                                tiers.append(f"{tier}: {pricing[tier]}")
                        price = " | ".join(tiers) if tiers else "N/A"
                    else:
                        price = "N/A"
                else:
                    price = str(pricing)

                print(f"    - {skill_name}: {price} GLUE")

            if len(skills) > 3:
                print(f"    ... ({len(skills) - 3} more skills)")
            print()
        except Exception as e:
            print(f"{RED}[ERROR]{RESET} Failed to parse AgentCard: {e}\n")
    else:
        print_test_result(f"{name} /.well-known/agent-card", endpoint, False, message)

    return results


def test_all_agents():
    """Test all agent endpoints"""
    print_section("AGENT ENDPOINTS")

    all_results = {}

    for name, url in AGENTS.items():
        print(f"{YELLOW}{name} Agent{RESET}")
        print("-" * 80)
        results = test_agent(name, url)
        all_results[name] = results

    return all_results


def print_summary(facilitator_results: Dict[str, bool], agent_results: Dict[str, Dict[str, bool]]):
    """Print test summary"""
    print_section("TEST SUMMARY")

    # Facilitator summary
    print(f"{CYAN}Facilitator:{RESET}")
    total_facilitator = len(facilitator_results)
    passing_facilitator = sum(facilitator_results.values())

    print(f"  Health: {'✓' if facilitator_results.get('health') else '✗'}")
    print(f"  Supported: {'✓' if facilitator_results.get('supported') else '✗'}")
    print(f"  Verify: {'✓' if facilitator_results.get('verify') else '✗'}")
    print(f"  Total: {passing_facilitator}/{total_facilitator} passing")
    print()

    # Agent summary
    print(f"{CYAN}Agents:{RESET}")
    for name, results in agent_results.items():
        total = len(results)
        passing = sum(results.values())
        status = f"{GREEN}✓{RESET}" if passing == total else f"{RED}✗{RESET}"

        health_status = "✓" if results.get('health') else "✗"
        card_status = "✓" if results.get('agent_card') else "✗"

        print(f"  {status} {name}: {passing}/{total} passing (health: {health_status}, card: {card_status})")

    print()

    # Overall status
    total_facilitator_tests = len(facilitator_results)
    passing_facilitator_tests = sum(facilitator_results.values())

    total_agent_tests = sum(len(results) for results in agent_results.values())
    passing_agent_tests = sum(sum(results.values()) for results in agent_results.values())

    total_tests = total_facilitator_tests + total_agent_tests
    passing_tests = passing_facilitator_tests + passing_agent_tests

    print(f"{CYAN}Overall:{RESET}")
    print(f"  Total endpoints tested: {total_tests}")
    print(f"  Passing: {passing_tests}")
    print(f"  Failing: {total_tests - passing_tests}")
    print()

    if passing_tests == total_tests:
        print(f"{GREEN}[SUCCESS] All endpoints responding!{RESET}")
        print()
        print(f"{BLUE}Stack is ready for transactions{RESET}")
        print()
        print(f"{CYAN}Next steps:{RESET}")
        print("  1. Test agent-to-agent purchases:")
        print(f"     python scripts/demo_client_purchases.py --production")
        print()
        print("  2. Run end-to-end integration test:")
        print(f"     python tests/test_level3_e2e.py --production")
        return 0
    else:
        print(f"{RED}[ERROR] Some endpoints not responding{RESET}")
        print()
        print(f"{CYAN}Troubleshooting:{RESET}")

        if not all(facilitator_results.values()):
            print("  Facilitator issues:")
            print("    - Check ECS service: facilitator-production (us-east-2)")
            print("    - Check CloudWatch logs")
            print("    - Verify domain: facilitator.ultravioletadao.xyz")

        failing_agents = [name for name, results in agent_results.items() if not all(results.values())]
        if failing_agents:
            print("  Agent issues:")
            for name in failing_agents:
                service_name = name.lower().replace(" ", "-")
                print(f"    - {name}: Check ECS service karmacadabra-prod-{service_name} (us-east-1)")

        print()
        print("  AWS ECS commands:")
        print("    # Facilitator (us-east-2):")
        print("    aws ecs describe-services --region us-east-2 --cluster facilitator-production --services \\")
        print("      facilitator-production")
        print()
        print("    # Karmacadabra agents (us-east-1):")
        print("    aws ecs describe-services --region us-east-1 --cluster karmacadabra-prod --services \\")
        print("      karmacadabra-prod-validator \\")
        print("      karmacadabra-prod-karma-hello \\")
        print("      karmacadabra-prod-abracadabra \\")
        print("      karmacadabra-prod-skill-extractor \\")
        print("      karmacadabra-prod-voice-extractor")

        return 1


def main():
    print(f"\n{BLUE}{'='*80}{RESET}")
    print(f"{BLUE}Testing All Karmacadabra Endpoints{RESET}")
    print(f"{BLUE}{'='*80}{RESET}")
    print()
    print(f"Facilitator: {FACILITATOR_URL}")
    print(f"Agents: *.{BASE_DOMAIN}")
    print(f"Protocol: HTTPS")
    print()

    # Test facilitator
    facilitator_results = test_facilitator()

    # Test all agents
    agent_results = test_all_agents()

    # Print summary
    return print_summary(facilitator_results, agent_results)


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print(f"\n{YELLOW}Test interrupted{RESET}")
        sys.exit(130)

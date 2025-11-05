/**
 * k6 Load Testing Script for x402 Facilitator
 *
 * Tests both /verify and /settle endpoints under load
 *
 * Installation:
 *   Windows: choco install k6
 *   Mac: brew install k6
 *   Linux: https://k6.io/docs/getting-started/installation/
 *
 * Usage:
 *   k6 run k6_load_test.js
 *   k6 run --vus 10 --duration 30s k6_load_test.js  # Custom load
 *
 * Scenarios:
 *   - verify_light: Low load (5 VUs for 1 min)
 *   - verify_medium: Medium load (20 VUs for 2 min)
 *   - verify_heavy: High load (50 VUs for 1 min)
 *   - settle_light: Settle transactions (2 VUs for 1 min)
 */

import http from 'k6/http';
import { check, group, sleep } from 'k6';
import { Rate, Trend } from 'k6/metrics';

// Custom metrics
const verifySuccessRate = new Rate('verify_success_rate');
const settleSuccessRate = new Rate('settle_success_rate');
const verifyLatency = new Trend('verify_latency');
const settleLatency = new Trend('settle_latency');

// Configuration
const FACILITATOR_URL = __ENV.FACILITATOR_URL || 'https://facilitator.ultravioletadao.xyz';

// Load test scenarios
export const options = {
  scenarios: {
    // Scenario 1: Light load on /verify
    verify_light: {
      executor: 'constant-vus',
      vus: 5,
      duration: '1m',
      gracefulStop: '10s',
      tags: { test_type: 'verify', load: 'light' },
    },

    // Scenario 2: Medium load on /verify
    verify_medium: {
      executor: 'ramping-vus',
      startVUs: 0,
      stages: [
        { duration: '30s', target: 20 },  // Ramp up to 20 VUs
        { duration: '1m', target: 20 },   // Stay at 20 VUs
        { duration: '30s', target: 0 },   // Ramp down
      ],
      startTime: '1m',  // Start after verify_light
      tags: { test_type: 'verify', load: 'medium' },
    },

    // Scenario 3: Heavy load spike on /verify
    verify_heavy: {
      executor: 'constant-vus',
      vus: 50,
      duration: '30s',
      startTime: '3m30s',  // Start after verify_medium
      tags: { test_type: 'verify', load: 'heavy' },
    },

    // Scenario 4: Light load on /settle (fewer VUs due to on-chain txs)
    settle_light: {
      executor: 'constant-vus',
      vus: 2,
      duration: '1m',
      startTime: '4m',  // Start after verify tests
      tags: { test_type: 'settle', load: 'light' },
    },
  },

  thresholds: {
    'http_req_duration': ['p(95)<500'],  // 95% of requests < 500ms
    'verify_success_rate': ['rate>0.99'], // 99% success rate for verify
    'settle_success_rate': ['rate>0.90'], // 90% success rate for settle (on-chain can fail)
    'http_req_failed': ['rate<0.01'],    // Less than 1% failures
  },
};

// Sample valid verify request (with dummy signature)
function createVerifyRequest() {
  const now = Math.floor(Date.now() / 1000);
  const nonce = '0x' + Array.from({length: 64}, () => Math.floor(Math.random() * 16).toString(16)).join('');
  const signature = '0x' + Array.from({length: 130}, () => Math.floor(Math.random() * 16).toString(16)).join('');

  return {
    x402Version: 1,
    paymentPayload: {
      x402Version: 1,
      scheme: 'exact',
      network: 'avalanche-fuji',
      payload: {
        signature: signature,
        authorization: {
          from: '0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8',
          to: '0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8',
          value: '10000',
          validAfter: 0,
          validBefore: now + 3600,
          nonce: nonce,
        },
      },
    },
    paymentRequirements: {
      network: 'avalanche-fuji',
      scheme: 'exact',
      asset: '0x3D19A80b3bD5CC3a4E55D4b5B753bC36d6A44743',
      recipient: '0x34033041a5944B8F10f8E4D8496Bfb84f1A293A8',
      amount: '10000',
    },
  };
}

// Test /health endpoint
export function testHealth() {
  group('Health Check', function () {
    const res = http.get(`${FACILITATOR_URL}/health`);

    check(res, {
      'health status is 200': (r) => r.status === 200,
      'health has providers': (r) => {
        const data = JSON.parse(r.body);
        return data.providers && data.providers.length > 0;
      },
    });
  });
}

// Test /supported endpoint
export function testSupported() {
  group('Supported Networks', function () {
    const res = http.get(`${FACILITATOR_URL}/supported`);

    check(res, {
      'supported status is 200': (r) => r.status === 200,
      'supported has kinds': (r) => {
        const data = JSON.parse(r.body);
        return data.kinds && data.kinds.length > 0;
      },
    });
  });
}

// Test /verify endpoint (this will reject invalid signatures but tests throughput)
export function testVerify() {
  group('Verify Payment', function () {
    const payload = createVerifyRequest();
    const startTime = Date.now();

    const res = http.post(
      `${FACILITATOR_URL}/verify`,
      JSON.stringify(payload),
      {
        headers: { 'Content-Type': 'application/json' },
        tags: { name: 'verify' },
      }
    );

    const duration = Date.now() - startTime;
    verifyLatency.add(duration);

    const success = check(res, {
      'verify status is 200': (r) => r.status === 200,
      'verify response is valid JSON': (r) => {
        try {
          JSON.parse(r.body);
          return true;
        } catch {
          return false;
        }
      },
    });

    verifySuccessRate.add(success);
  });
}

// Test /settle endpoint (with dummy signature - will fail but tests endpoint)
export function testSettle() {
  group('Settle Payment', function () {
    const payload = createVerifyRequest();  // Same structure
    const startTime = Date.now();

    const res = http.post(
      `${FACILITATOR_URL}/settle`,
      JSON.stringify(payload),
      {
        headers: { 'Content-Type': 'application/json' },
        tags: { name: 'settle' },
      }
    );

    const duration = Date.now() - startTime;
    settleLatency.add(duration);

    // Settle will fail with invalid signature, but should return 200 with error
    const success = check(res, {
      'settle responds (200 or 422)': (r) => r.status === 200 || r.status === 422,
      'settle response is valid JSON': (r) => {
        try {
          JSON.parse(r.body);
          return true;
        } catch {
          return false;
        }
      },
    });

    settleSuccessRate.add(success);
  });
}

// Main execution function
export default function () {
  const testType = __ENV.TEST_TYPE || 'verify';

  if (testType === 'verify') {
    testVerify();
  } else if (testType === 'settle') {
    testSettle();
  } else {
    // Run all tests in sequence
    testHealth();
    testSupported();
    testVerify();
  }

  sleep(1);  // 1 second between iterations
}

// Setup function (runs once at start)
export function setup() {
  console.log('🚀 Starting x402 facilitator load tests...');
  console.log(`   Target: ${FACILITATOR_URL}`);

  // Verify facilitator is up
  const healthRes = http.get(`${FACILITATOR_URL}/health`);
  if (healthRes.status !== 200) {
    throw new Error(`Facilitator not healthy: ${healthRes.status}`);
  }

  console.log('✅ Facilitator is healthy');
  return {};
}

// Teardown function (runs once at end)
export function teardown(data) {
  console.log('✅ Load tests complete!');
}

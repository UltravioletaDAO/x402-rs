#!/usr/bin/env node
/**
 * mock_rpc.js -- RPC JSON-RPC simulado para el banco de capacidad del facilitador.
 *
 * Por que existe: el paso 5 de la secuencia de la auditoria pide medir capacidad de
 * CPU/pooling "con RPCs simulados". Un RPC real mete latencia de red y de bloque en la
 * medicion y hace que el numero dependa del proveedor, no del binario.
 *
 * NO habla con ninguna cadena. Todas las respuestas son sinteticas.
 *
 * Rutas:
 *   POST /evm/<chainId>   JSON-RPC EVM (eth_chainId responde ese chainId)
 *   GET  /__stats         contadores por metodo
 *   POST /__reset         pone los contadores a cero
 *
 * Latencia (variables de entorno):
 *   MOCK_RPC_LATENCY_MS         latencia base de toda respuesta (default 0)
 *   MOCK_RPC_JITTER_MS          jitter uniforme [0,J) sumado a la base (default 0)
 *   MOCK_RPC_LATENCY_<METODO>   override por metodo, en mayusculas
 *                               p.ej. MOCK_RPC_LATENCY_ETH_CALL=25
 *   MOCK_RPC_RECEIPT_DELAY_MS   ms entre eth_sendRawTransaction y el primer recibo
 *                               no nulo de ESA transaccion (default 0)
 *
 * Uso:
 *   MOCK_RPC_LATENCY_ETH_CALL=25 node scripts/bench/mock_rpc.js --port 8545
 */
'use strict';
const http = require('http');
const crypto = require('crypto');

const args = process.argv.slice(2);
const argPort = args.indexOf('--port');
const PORT = argPort >= 0 ? Number(args[argPort + 1]) : Number(process.env.MOCK_RPC_PORT || 8545);

const BASE_MS = Number(process.env.MOCK_RPC_LATENCY_MS || 0);
const JITTER_MS = Number(process.env.MOCK_RPC_JITTER_MS || 0);
const RECEIPT_DELAY_MS = Number(process.env.MOCK_RPC_RECEIPT_DELAY_MS || 0);

function methodLatency(method) {
  const key = 'MOCK_RPC_LATENCY_' + method.replace(/([a-z0-9])([A-Z])/g, '$1_$2').toUpperCase();
  const v = process.env[key];
  const base = v !== undefined ? Number(v) : BASE_MS;
  return JITTER_MS > 0 ? base + Math.random() * JITTER_MS : base;
}

const stats = Object.create(null);
const submitted = new Map(); // txHash -> timestamp de aceptacion

// Bloque 0 al arrancar; avanza 1 cada 2s, como una L2 rapida.
const START = Date.now();
const BLOCK_MS = Number(process.env.MOCK_RPC_BLOCK_MS || 2000);
const blockNumber = () => 0x1000000 + Math.floor((Date.now() - START) / BLOCK_MS);
const hex = (n) => '0x' + BigInt(n).toString(16);
const word = (h) => h.replace(/^0x/, '').padStart(64, '0');

// ABI-encode de un string dinamico (offset + longitud + datos), para name()/version().
function abiString(s) {
  const b = Buffer.from(s, 'utf8');
  const pad = Buffer.alloc(Math.ceil(b.length / 32) * 32);
  b.copy(pad);
  return '0x' + word('0x20') + word('0x' + b.length.toString(16)) + pad.toString('hex');
}

const SELECTORS = {
  '70a08231': () => '0x' + word('0xffffffffffffffff'),      // balanceOf(address) -> saldo alto
  '06fdde03': () => abiString('USDC'),                       // name()
  '54fd4d50': () => abiString('2'),                          // version()
  '18160ddd': () => '0x' + word('0x0'),                      // totalSupply()
  'e94a0102': () => '0x' + word('0x0'),                      // authorizationState(address,bytes32)
  '313ce567': () => '0x' + word('0x6'),                      // decimals()
};

function ethCall(params) {
  // alloy serializa el calldata como `input`; otros clientes usan `data`.
  // Leer solo uno de los dos devuelve '0x' a todo y el facilitador responde
  // ZeroData("balanceOf") -- el primer sintoma que dio este mock.
  const p0 = (params && params[0]) || {};
  const data = p0.input || p0.data || '0x';
  const sel = data.slice(2, 10).toLowerCase();
  const fn = SELECTORS[sel];
  // transferWithAuthorization y demas escrituras simuladas devuelven vacio, que es
  // lo que alloy interpreta como "la simulacion no revirtio".
  return fn ? fn() : '0x';
}

function receiptFor(hash) {
  const acceptedAt = submitted.get(hash);
  if (acceptedAt === undefined) return null;
  if (Date.now() - acceptedAt < RECEIPT_DELAY_MS) return null;
  const bn = blockNumber();
  return {
    transactionHash: hash,
    transactionIndex: '0x0',
    blockHash: '0x' + word(hex(bn)),
    blockNumber: hex(bn),
    from: '0x0000000000000000000000000000000000000001',
    to: '0x036cbd53842c5426634e7929541ec2318f3dcf7e',
    cumulativeGasUsed: '0x1d4c0',
    gasUsed: '0x1d4c0',
    effectiveGasPrice: '0x3b9aca00',
    contractAddress: null,
    logs: [],
    logsBloom: '0x' + '0'.repeat(512),
    status: '0x1',
    type: '0x2',
  };
}

function answer(method, params, chainId) {
  switch (method) {
    case 'eth_chainId': return hex(chainId);
    case 'net_version': return String(chainId);
    case 'eth_blockNumber': return hex(blockNumber());
    case 'eth_getTransactionCount': return '0x0';
    case 'eth_gasPrice': return '0x3b9aca00';
    case 'eth_maxPriorityFeePerGas': return '0x3b9aca00';
    case 'eth_estimateGas': return '0x1d4c0';
    case 'eth_getCode': return '0x';
    case 'eth_getBalance': return '0xde0b6b3a7640000';
    case 'eth_getLogs': return [];
    case 'eth_call': return ethCall(params);
    case 'eth_sendRawTransaction': {
      const raw = (params && params[0]) || '';
      const h = '0x' + crypto.createHash('sha256').update(String(raw)).digest('hex');
      if (!submitted.has(h)) submitted.set(h, Date.now());
      return h;
    }
    case 'eth_getTransactionReceipt': return receiptFor((params && params[0]) || '');
    case 'eth_getTransactionByHash': {
      const h = (params && params[0]) || '';
      if (!submitted.has(h)) return null;
      return { hash: h, blockNumber: hex(blockNumber()), blockHash: '0x' + word(hex(blockNumber())),
               from: '0x0000000000000000000000000000000000000001', to: null, value: '0x0',
               gas: '0x1d4c0', gasPrice: '0x3b9aca00', nonce: '0x0', input: '0x', type: '0x2',
               transactionIndex: '0x0', chainId: hex(chainId), v: '0x0', r: '0x' + word('0x1'), s: '0x' + word('0x1') };
    }
    case 'eth_getBlockByNumber':
    case 'eth_getBlockByHash': {
      const bn = blockNumber();
      return { number: hex(bn), hash: '0x' + word(hex(bn)), parentHash: '0x' + word(hex(bn - 1)),
               timestamp: hex(Math.floor(Date.now() / 1000)), baseFeePerGas: '0x3b9aca00',
               gasLimit: '0x1c9c380', gasUsed: '0x0', miner: '0x' + '0'.repeat(40),
               difficulty: '0x0', totalDifficulty: '0x0', extraData: '0x', size: '0x0',
               transactions: [], uncles: [], logsBloom: '0x' + '0'.repeat(512),
               sha3Uncles: '0x' + word('0x0'), stateRoot: '0x' + word('0x0'),
               transactionsRoot: '0x' + word('0x0'), receiptsRoot: '0x' + word('0x0'),
               mixHash: '0x' + word('0x0'), nonce: '0x0000000000000000' };
    }
    case 'eth_feeHistory':
      return { oldestBlock: hex(blockNumber()), baseFeePerGas: ['0x3b9aca00', '0x3b9aca00'],
               gasUsedRatio: [0.5], reward: [['0x3b9aca00']] };
    case 'eth_syncing': return false;
    default: return null;
  }
}

const server = http.createServer((req, res) => {
  if (req.method === 'GET' && req.url.startsWith('/__stats')) {
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end(JSON.stringify({ methods: stats, submitted: submitted.size, block: blockNumber() }));
  }
  if (req.method === 'POST' && req.url.startsWith('/__reset')) {
    for (const k of Object.keys(stats)) delete stats[k];
    submitted.clear();
    res.writeHead(200, { 'content-type': 'application/json' });
    return res.end('{"ok":true}');
  }
  const m = /^\/evm\/(\d+)/.exec(req.url || '');
  const chainId = m ? Number(m[1]) : 84532;
  let body = '';
  req.on('data', (c) => { body += c; });
  req.on('end', () => {
    let parsed;
    try { parsed = JSON.parse(body || '{}'); } catch { parsed = {}; }
    const one = (r) => {
      const method = (r && r.method) || '';
      stats[method] = (stats[method] || 0) + 1;
      return { jsonrpc: '2.0', id: r && r.id !== undefined ? r.id : 1,
               result: answer(method, r && r.params, chainId) };
    };
    const reqs = Array.isArray(parsed) ? parsed : [parsed];
    const out = Array.isArray(parsed) ? reqs.map(one) : one(parsed);
    const delay = Math.max(...reqs.map((r) => methodLatency((r && r.method) || '')), 0);
    const send = () => {
      const payload = JSON.stringify(out);
      res.writeHead(200, { 'content-type': 'application/json', 'content-length': Buffer.byteLength(payload) });
      res.end(payload);
    };
    if (delay > 0) setTimeout(send, delay); else send();
  });
});
server.keepAliveTimeout = 65000;
server.headersTimeout = 66000;
server.listen(PORT, '127.0.0.1', () => {
  process.stderr.write(`mock_rpc listening on http://127.0.0.1:${PORT} base=${BASE_MS}ms jitter=${JITTER_MS}ms receipt_delay=${RECEIPT_DELAY_MS}ms\n`);
});

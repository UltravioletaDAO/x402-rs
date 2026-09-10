#!/usr/bin/env node
/**
 * loadgen.js -- generador de carga de modelo abierto para el banco del facilitador.
 *
 * Por que no k6/oha/vegeta para la mezcla: todas las rutas calientes del facilitador
 * llevan un limitador por IP (tower_governor + SmartIpKeyExtractor). Un generador que
 * manda todo desde una sola IP mide el limitador, no el servicio. Este rota
 * X-Forwarded-For sobre un pool de IPs sinteticas, que es lo que hace el trafico real:
 * muchos clientes distintos. `--clients 1` reproduce el otro caso, un solo cliente.
 *
 * Modelo abierto (llegadas a tasa fija) y no cerrado (N usuarios en bucle): con modelo
 * cerrado la tasa cae sola cuando el servidor se pone lento y el p99 se ve mejor de lo
 * que es. El desfase de llegada (schedule lag) se reporta para saber si el generador
 * fue el cuello de botella.
 *
 * Uso:
 *   node scripts/bench/loadgen.js --base http://127.0.0.1:8080 --mix mix.json \
 *     --rps 100 --duration 60 --clients 64 --out resultados.json
 *
 * Formato de --mix (JSON):
 *   [{"name":"supported","weight":8.5,"method":"GET","path":"/supported"},
 *    {"name":"verify","weight":3.7,"method":"POST","path":"/verify","bodyFile":"verify.json"}]
 */
'use strict';
const http = require('http');
const fs = require('fs');

function arg(name, def) {
  const i = process.argv.indexOf('--' + name);
  return i >= 0 ? process.argv[i + 1] : def;
}
const BASE = arg('base', 'http://127.0.0.1:8080');
const RPS = Number(arg('rps', '50'));
const DURATION = Number(arg('duration', '30'));
const CLIENTS = Number(arg('clients', '64'));
const MIXFILE = arg('mix', null);
const OUT = arg('out', null);
const WARMUP = Number(arg('warmup', '0'));      // segundos descartados del resultado
const TIMEOUT = Number(arg('timeout', '120')) * 1000;

const u = new URL(BASE);
const HOST = u.hostname, PORT = Number(u.port || 80);

const mix = JSON.parse(fs.readFileSync(MIXFILE, 'utf8'));
for (const r of mix) {
  if (r.bodyFile) r.body = fs.readFileSync(r.bodyFile, 'utf8');
  r.samples = [];      // [ms] solo despues del warmup
  r.status = {};       // status -> n
  r.sent = 0; r.done = 0; r.bytes = 0; r.errors = 0;
}
const totalWeight = mix.reduce((a, r) => a + r.weight, 0);
function pick() {
  let x = Math.random() * totalWeight;
  for (const r of mix) { x -= r.weight; if (x <= 0) return r; }
  return mix[mix.length - 1];
}

// Pool de IPs sinteticas. 10.x.x.x, una por "cliente".
const ips = Array.from({ length: CLIENTS }, (_, i) =>
  `10.${(i >> 16) & 255}.${(i >> 8) & 255}.${i & 255}`);

// Un agente por cliente: cada IP sintetica usa su propio pool de conexiones, como
// clientes distintos de verdad.
const agents = ips.map(() => new http.Agent({ keepAlive: true, maxSockets: 64, keepAliveMsecs: 30000 }));

let t0 = Date.now();
let warmDone = WARMUP <= 0;
let inflight = 0, maxInflight = 0, scheduled = 0;
let lagSum = 0, lagMax = 0, lagN = 0;

function fire(route, clientIdx) {
  const opts = {
    host: HOST, port: PORT, method: route.method, path: route.path,
    agent: agents[clientIdx],
    headers: { 'x-forwarded-for': ips[clientIdx], 'connection': 'keep-alive', 'accept': '*/*' },
  };
  if (route.body) {
    opts.headers['content-type'] = 'application/json';
    opts.headers['content-length'] = Buffer.byteLength(route.body);
  }
  const started = process.hrtime.bigint();
  route.sent++; inflight++; if (inflight > maxInflight) maxInflight = inflight;
  const req = http.request(opts, (res) => {
    let n = 0;
    res.on('data', (c) => { n += c.length; });
    res.on('end', () => {
      const ms = Number(process.hrtime.bigint() - started) / 1e6;
      inflight--; route.done++; route.bytes += n;
      route.status[res.statusCode] = (route.status[res.statusCode] || 0) + 1;
      if (warmDone) route.samples.push(ms);
    });
  });
  req.setTimeout(TIMEOUT, () => { req.destroy(new Error('timeout')); });
  req.on('error', () => {
    inflight--; route.done++; route.errors++;
    route.status['ERR'] = (route.status['ERR'] || 0) + 1;
  });
  if (route.body) req.write(route.body);
  req.end();
}

// Programa las llegadas por lotes de 10 ms para no gastar un timer por request.
const TICK_MS = 10;
const perTick = RPS * TICK_MS / 1000;
let carry = 0, client = 0;
let timer = null;

/**
 * Abre una conexion por cliente antes de medir.
 *
 * Sin esto, a tasas bajas la mayoria de los clientes hace su PRIMERA request
 * dentro de la ventana medida y paga el TCP connect ahi: el p95 de una ruta de
 * 0,6 ms daba 300-900 ms, que era el handshake y no el servidor.
 */
function prewarm(done) {
  let left = CLIENTS;
  if (!left) return done();
  for (let i = 0; i < CLIENTS; i++) {
    const req = http.request({ host: HOST, port: PORT, method: 'GET', path: '/health',
      agent: agents[i], headers: { 'x-forwarded-for': ips[i], 'connection': 'keep-alive' } },
      (res) => { res.resume(); res.on('end', () => { if (--left === 0) done(); }); });
    req.on('error', () => { if (--left === 0) done(); });
    req.end();
  }
}

function startClock() {
  t0 = Date.now();
  timer = setInterval(tick, TICK_MS);
}

function tick() {
  const elapsed = (Date.now() - t0) / 1000;
  if (!warmDone && elapsed >= WARMUP) warmDone = true;
  if (elapsed >= WARMUP + DURATION) { clearInterval(timer); return finish(); }
  const expected = Math.floor((elapsed * RPS));
  const lag = expected - scheduled;
  if (lag > 0) { lagSum += lag; lagN++; if (lag > lagMax) lagMax = lag; }
  carry += perTick;
  let n = Math.floor(carry); carry -= n;
  for (let i = 0; i < n; i++) {
    fire(pick(), client % CLIENTS); client++; scheduled++;
  }
}

prewarm(startClock);

function pct(sorted, p) {
  if (!sorted.length) return null;
  const i = Math.min(sorted.length - 1, Math.ceil(p / 100 * sorted.length) - 1);
  return sorted[i];
}

function finish() {
  // Deja terminar lo que quedo en vuelo.
  const deadline = Date.now() + TIMEOUT + 2000;
  const wait = setInterval(() => {
    if (inflight === 0 || Date.now() > deadline) {
      clearInterval(wait);
      report();
    }
  }, 100);
}

function report() {
  const wall = (Date.now() - t0) / 1000 - WARMUP;
  const out = { base: BASE, targetRps: RPS, durationSec: DURATION, warmupSec: WARMUP,
                clients: CLIENTS, wallSec: Number(wall.toFixed(3)),
                maxInflight, scheduleLagMean: lagN ? Number((lagSum / lagN).toFixed(1)) : 0,
                scheduleLagMax: lagMax, routes: {} };
  let totDone = 0, totSamples = 0;
  for (const r of mix) {
    const s = r.samples.slice().sort((a, b) => a - b);
    totDone += r.done; totSamples += s.length;
    out.routes[r.name] = {
      method: r.method, path: r.path, weight: r.weight,
      sent: r.sent, done: r.done, errors: r.errors, status: r.status,
      measured: s.length,
      rps: Number((s.length / wall).toFixed(2)),
      bytes: r.bytes,
      p50: s.length ? Number(pct(s, 50).toFixed(2)) : null,
      p90: s.length ? Number(pct(s, 90).toFixed(2)) : null,
      p95: s.length ? Number(pct(s, 95).toFixed(2)) : null,
      p99: s.length ? Number(pct(s, 99).toFixed(2)) : null,
      max: s.length ? Number(s[s.length - 1].toFixed(2)) : null,
      mean: s.length ? Number((s.reduce((a, b) => a + b, 0) / s.length).toFixed(2)) : null,
    };
  }
  const all = mix.flatMap((r) => r.samples).sort((a, b) => a - b);
  out.total = { done: totDone, measured: totSamples, rps: Number((totSamples / wall).toFixed(2)),
                p50: all.length ? Number(pct(all, 50).toFixed(2)) : null,
                p95: all.length ? Number(pct(all, 95).toFixed(2)) : null,
                p99: all.length ? Number(pct(all, 99).toFixed(2)) : null };
  const txt = JSON.stringify(out, null, 1);
  if (OUT) fs.writeFileSync(OUT, txt);
  process.stdout.write(txt + '\n');
  process.exit(0);
}

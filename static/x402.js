// ---------------------------------------------------------------------------
// Presentacion de redes del facilitador x402: nombre, icono y explorador.
// La consumen la portada, /networks y el uso en linea dentro de la prosa.
//
// Sale de GET /networks.json, que el facilitador arma con lo que /supported
// sirve (src/networks_json.rs). En static/ no se tipea ningun icono ni
// explorador -- lo revisa `static_types_no_explorer_and_no_icon` --, asi que
// una red que el facilitador empieza a servir aparece aca sin tocar nada.
//
// loadNetworks() pide el documento una vez. Mientras no respondio, o si fallo,
// networkIcon() da null y el chip sale con monograma: nunca un icono inventado.
// ---------------------------------------------------------------------------
let REDES = null;        // Map: id y caip2 -> fila de /networks.json
let ICONOS_TOKEN = null; // Map: simbolo en minuscula -> ruta del icono, o null
let cargaDeRedes = null;

// Solo la ruta: la pagina carga el icono de su propio origen (localhost,
// staging, produccion), sea cual sea el host que nombra /networks.json.
function localPath(url) {
  const m = typeof url === 'string' && /^(?:[a-z][a-z0-9+.-]*:\/\/[^/?#]*)?(\/[^?#]*)$/i.exec(url);
  return m ? m[1] : null;
}

function indexNetworks(body) {
  if (!body || !Array.isArray(body.networks)) throw new Error('Invalid networks catalog');
  const redes = new Map(), tokens = new Map();
  body.networks.forEach(row => {
    if (!row || typeof row !== 'object') return;
    [row.id, row.caip2].forEach(n => { if (typeof n === 'string') redes.set(n, row); });
    (Array.isArray(row.tokens) ? row.tokens : []).forEach(t => {
      const k = String(t?.symbol || '').toLowerCase();
      if (k && !tokens.get(k)) tokens.set(k, localPath(t.icon));
    });
  });
  REDES = redes;
  ICONOS_TOKEN = tokens;
  return redes;
}

// Una sola peticion por pagina; si falla, la proxima llamada reintenta.
function loadNetworks() {
  if (!cargaDeRedes) {
    cargaDeRedes = fetch('/networks.json')
      .then(r => { if (!r.ok) throw new Error('Networks catalog unavailable'); return r.json(); })
      .then(indexNetworks)
      .catch(e => { cargaDeRedes = null; throw e; });
  }
  return cargaDeRedes;
}

function networkOf(name) { return REDES?.get(String(name || '')) || null; }
function networkIcon(name) { return localPath(networkOf(name)?.icon); }
function tokenIcon(symbol) { return ICONOS_TOKEN?.get(String(symbol || '').toLowerCase()) || null; }

// El enlace al explorador de una red, desde su plantilla. Null si la red o la
// plantilla no se conocen: un enlace adivinado a un explorador muerto hace que
// una transaccion real parezca inventada.
function explorerUrl(name, kind, value) {
  const template = networkOf(name)?.explorer?.[kind];
  if (typeof template !== 'string' || value == null || String(value).trim() === '') return null;
  return template.replace('{' + kind + '}', encodeURIComponent(String(value).trim()));
}

// Completa lo que en el HTML solo NOMBRA una red o un token:
//   <img data-net-icon="base">                               -> src
//   <img data-token-icon="usdc">                             -> src
//   <a data-explorer="solana">F742...</a>                    -> href (la direccion es el texto)
//   <div data-explorer="base" data-explorer-address="0x..."> -> el click abre el explorador
// Sin direccion no hay enlace: `data-explorer-fee-payer` pide que la pagina la
// complete con el feePayer que publica /supported antes de llamar esto.
// El src se escribe como ruta ("/<icono>.png"), la forma que miden las reglas
// `.network-logo[src=...]` de la portada.
function hydrateNetworks(root) {
  const scope = root || document;
  scope.querySelectorAll('img[data-net-icon]').forEach(img => {
    const src = networkIcon(img.dataset.netIcon);
    if (src) img.setAttribute('src', src);
  });
  scope.querySelectorAll('img[data-token-icon]').forEach(img => {
    const src = tokenIcon(img.dataset.tokenIcon);
    if (src) img.setAttribute('src', src);
  });
  scope.querySelectorAll('[data-explorer]').forEach(el => {
    // Only a link's own text is an address; a card's text is its whole face.
    const address = el.dataset.explorerAddress || (el.tagName === 'A' ? el.textContent : '');
    const url = explorerUrl(el.dataset.explorer, 'address', address);
    if (!url) return;
    if (el.tagName === 'A') { el.href = url; el.rel = 'noopener'; }
    else el.onclick = () => window.open(url, '_blank', 'noopener');
  });
}

// Familia -> monograma, para una red que /networks.json todavia no describe. La
// clave es el namespace CAIP-2: es lo unico legible de un identificador
// desconocido sin adivinar.
const MONO_FAMILIA = {
  "eip155": "EV", "solana": "SO", "near": "NE", "stellar": "ST",
  "xrpl": "XR", "fogo": "FO", "algorand": "AL", "sui": "SU", "hedera": "HE"
};

function monogramaDeRed(nombre){
  const s = String(nombre || "");
  if (s.includes(":")) {
    const ns = s.split(":")[0];
    return MONO_FAMILIA[ns] || ns.replace(/[^a-z0-9]/gi, "").slice(0, 2).toUpperCase() || "??";
  }
  return s.replace(/[^a-z0-9]/gi, "").slice(0, 2).toUpperCase() || "??";
}

const escHtml = s => String(s).replace(/[&<>"']/g, c =>
  ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" }[c]));

// El unico constructor de icono del sitio. Devuelve HTML y NUNCA devuelve vacio.
//
// El monograma va SIEMPRE en el markup y la imagen ENCIMA. Asi funciona con
// JavaScript apagado y con un PNG caido: el <img> queda transparente sobre el
// <b> y algo se lee igual. La version anterior emitia <img> O monograma y
// dependia de onerror + JS -- el mismo agujero que vino a tapar.
// (Riesgo descartado con evidencia: curl -sSI / no devuelve
// Content-Security-Policy, asi que el onerror en linea no estaba bloqueado.
// No era ese el problema: se quita porque sobra.)
//
// `src` es la ruta del icono (networkIcon/tokenIcon), o null.
// `extra` admite "chip-red--tabla" o "chip-red--linea".
function chip(rotulo, src, extra){
  const cls = "chip-red" + (extra ? " " + extra : "");
  const t   = escHtml(rotulo);
  return '<span class="' + cls + '" title="' + t + '">' +
         '<b>' + escHtml(monogramaDeRed(rotulo)) + '</b>' +
         (src ? '<img src="' + escHtml(src) + '" alt="" width="96" height="96">' : '') +
         '</span>';
}

function chipRed(nombre, extra){ return chip(nombre, networkIcon(nombre), extra); }

// Un token sin PNG NO saca monograma: "US" seria el mismo para usdc, usdt y
// usdg. Devuelve cadena vacia y el simbolo en texto -- que siempre esta al
// lado -- lo dice.
function chipToken(sim, extra){
  const src = tokenIcon(sim);
  return src ? chip(sim, src, extra) : "";
}

// Merge only aliases the facilitator declares. This also retains v2-only
// networks; dropping every identifier containing ':' loses real capabilities.
function supportedCatalog(data) {
  if (!data || !Array.isArray(data.kinds)) throw new Error('Invalid supported catalog');
  const kinds = data.kinds.filter(k => k && typeof k.network === 'string' && typeof k.scheme === 'string');
  const parents = new Map();
  const find = n => {
    if (!parents.has(n)) parents.set(n, n);
    if (parents.get(n) !== n) parents.set(n, find(parents.get(n)));
    return parents.get(n);
  };
  const aliasesOf = k => [k.network, ...(Array.isArray(k.networkAliases) ? k.networkAliases.filter(n => typeof n === 'string') : [])];
  kinds.forEach(k => aliasesOf(k).forEach(n => parents.set(find(n), find(k.network))));
  const groups = new Map();
  kinds.forEach(k => {
    const id = find(k.network);
    if (!groups.has(id)) groups.set(id, {aliases: new Set(), schemes: new Set(), tokens: new Set(), kinds: []});
    const row = groups.get(id);
    aliasesOf(k).forEach(n => row.aliases.add(n));
    row.schemes.add(k.scheme);
    row.kinds.push(k);
    (Array.isArray(k.extra?.tokens) ? k.extra.tokens : []).forEach(t => {
      if (typeof t?.token === 'string') row.tokens.add(t.token.toLowerCase());
    });
  });
  return [...groups.values()].map(row => {
    const aliases = [...row.aliases].sort();
    row.name = aliases.find(n => !n.includes(':')) || aliases[0];
    row.testnet = aliases.some(n => /(testnet|sepolia|devnet|fuji|amoy|alfajores|holesky|baklava)/i.test(n));
    return row;
  }).sort((a, b) => Number(a.testnet) - Number(b.testnet) || a.name.localeCompare(b.name));
}

// Only presentation identities belong here. Payment tokens come from /supported.
function landingNetworkName(key = '') {
  const names = {
    'ethereum-testnet': 'ethereum-sepolia', 'base-testnet': 'base-sepolia',
    'avalanche-testnet': 'avalanche-fuji', 'polygon-testnet': 'polygon-amoy',
    'arbitrum-testnet': 'arbitrum-sepolia', 'optimism-testnet': 'optimism-sepolia',
    'celo-testnet': 'celo-sepolia', 'unichain-testnet': 'unichain-sepolia',
    'skale-mainnet': 'skale-base', 'skale-testnet': 'skale-base-sepolia',
    'hedera-mainnet': 'hedera:mainnet', 'hedera-testnet': 'hedera:testnet'
  };
  return names[key] || key.replace(/-mainnet$/, '');
}

function stablecoinsForCard(row) {
  if (!row) return null;
  const exact = row.kinds.filter(k => k.scheme === 'exact');
  if (!exact.some(k => Array.isArray(k.extra?.tokens))) return null;
  return [...new Set(exact.flatMap(k => Array.isArray(k.extra?.tokens) ? k.extra.tokens : [])
    .map(t => String(t?.token).toLowerCase()).filter(t => tokenIcon(t)))].sort();
}

function matchesStablecoinFilter(row, token) {
  return Boolean(row?.schemes.has('exact') && (!token || stablecoinsForCard(row)?.includes(token)));
}

// Chain health from GET /health/ready, keyed by both spellings of each chain:
// native Hedera appears in /supported under its CAIP-2 id only. Each entry also
// keeps the fewest settles any of its signers can still pay for. Null when the
// body is not a readiness answer (a 429, `probe_failed`, garbage), because a
// state nobody could read is shown as nothing, never as unhealthy.
function readinessIndex(body) {
  if (!body || !Array.isArray(body.networks)) return null;
  const index = new Map();
  body.networks.forEach(n => {
    if (!n || typeof n.status !== 'string') return;
    const settles = (Array.isArray(n.signers) ? n.signers : [])
      .map(s => s?.settlesRemaining).filter(Number.isFinite);
    const entry = {
      status: n.status,
      reason: typeof n.reason === 'string' ? n.reason : '',
      fewestSettles: settles.length ? Math.min(...settles) : null,
    };
    [n.network, n.caip2].forEach(name => { if (typeof name === 'string') index.set(name, entry); });
  });
  return index;
}

// The owner's line for the landing's dot: fewer than 10 settles left. Above it a
// thin signer (`degraded` / `signer_gas_low`) is an operator matter, not a
// visitor's: /health/ready and the balance alarms still report it.
const DOT_BELOW_SETTLES = 10;

// What a landing card shows for its chain: null while it is ok, unprobed, or
// only low on gas with 10 or more settles left; otherwise the label of its dot
// ("down: signer_gas_critical"), with the reason /health/ready published.
// `words` names the state in the page's language; the reason stays a token.
function cardHealth(index, key, words) {
  const entry = index?.get(landingNetworkName(key));
  if (!entry || (entry.status !== 'degraded' && entry.status !== 'down')) return null;
  const nearlyDry = entry.fewestSettles !== null && entry.fewestSettles < DOT_BELOW_SETTLES;
  if (entry.status === 'degraded' && entry.reason === 'signer_gas_low' && !nearlyDry) return null;
  const state = words?.[entry.status] || entry.status;
  return {status: entry.status, label: entry.reason ? `${state}: ${entry.reason}` : state};
}

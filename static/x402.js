// ---------------------------------------------------------------------------
// Iconografia de redes del facilitador x402. Bloque para static/x402.js.
// UNICA declaracion del sitio: la consumen la pared de la portada, la primera
// columna de /networks y el uso en linea dentro de la prosa.
//
// 84 claves = las 42 redes del enum Network por sus 2 grafias (v1 y CAIP-2).
// Derivado de Z:/ultravioleta/dao/x402-rs/src/network.rs (#[serde(rename)] +
// to_caip2()), medido 2026-09-03 contra /supported (78 vivas, subconjunto exacto).
//
// null      = la red esta declarada y el PNG todavia no existe.
// undefined = red que este mapa no conoce.
// Los dos casos van al monograma. La diferencia es documental y la revisa un test.
// ---------------------------------------------------------------------------
const ICONO_DE_RED = {
  "arc": "arc", "arc-testnet": "arc", "eip155:5042": "arc", "eip155:5042002": "arc",
  "hedera:mainnet": "hedera", "hedera:testnet": "hedera",
  "algorand": "algorand", "algorand-testnet": "algorand", "algorand:mainnet": "algorand", "algorand:testnet": "algorand",
  "arbitrum": "arbitrum", "arbitrum-sepolia": "arbitrum", "eip155:42161": "arbitrum", "eip155:421614": "arbitrum",
  "avalanche": "avalanche", "avalanche-fuji": "avalanche", "eip155:43113": "avalanche", "eip155:43114": "avalanche",
  "base": "base", "base-sepolia": "base", "eip155:8453": "base", "eip155:84532": "base",
  "bsc": "bsc", "eip155:56": "bsc",
  "celo": "celo", "celo-sepolia": "celo", "eip155:42220": "celo", "eip155:11142220": "celo",
  "ethereum": "ethereum", "ethereum-sepolia": "ethereum", "eip155:1": "ethereum", "eip155:11155111": "ethereum",
  "fogo": "fogo", "fogo-testnet": "fogo", "fogo:mainnet": "fogo", "fogo:testnet": "fogo",
  "hyperevm": "hyperevm", "hyperevm-testnet": "hyperevm", "eip155:333": "hyperevm", "eip155:999": "hyperevm",
  "monad": "monad", "eip155:143": "monad",
  "near": "near", "near-testnet": "near", "near:mainnet": "near", "near:testnet": "near",
  "optimism": "optimism", "optimism-sepolia": "optimism", "eip155:10": "optimism", "eip155:11155420": "optimism",
  "polygon": "polygon", "polygon-amoy": "polygon", "eip155:137": "polygon", "eip155:80002": "polygon",
  "robinhood": "robinhood", "robinhood-testnet": "robinhood", "eip155:4663": "robinhood", "eip155:46630": "robinhood",
  "scroll": "scroll", "eip155:534352": "scroll",
  "skale-base": "skale", "skale-base-sepolia": "skale", "eip155:1187947933": "skale", "eip155:324705682": "skale",
  "solana": "solana", "solana-devnet": "solana", "solana:5eykt4UsFv8P8NJdTREpY1vzqKqZKvdp": "solana", "solana:EtWTRABZaYq6iMfeYKouRu166VU2xqa1": "solana",
  "stellar": "stellar", "stellar-testnet": "stellar", "stellar:pubnet": "stellar", "stellar:testnet": "stellar",
  "sui": "sui", "sui-testnet": "sui", "sui:mainnet": "sui", "sui:testnet": "sui",
  "unichain": "unichain", "unichain-sepolia": "unichain", "eip155:130": "unichain", "eip155:1301": "unichain",
  "xrpl": "xrpl", "xrpl-testnet": "xrpl", "xrpl:0": "xrpl", "xrpl:1": "xrpl",
  // Declaradas en src/network.rs y sin PNG todavia. Salen con monograma, a proposito.
  "sei": null, "sei-testnet": null, "xdc": null,
  "eip155:1329": null, "eip155:1328": null, "eip155:50": null
};

// Familia -> monograma, para una red que el mapa todavia no conoce. La clave es el
// namespace CAIP-2: es lo unico legible de un identificador desconocido sin adivinar.
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
// `extra` admite "chip-red--tabla" o "chip-red--linea".
function chip(rotulo, ico, extra){
  const cls = "chip-red" + (extra ? " " + extra : "");
  const t   = escHtml(rotulo);
  return '<span class="' + cls + '" title="' + t + '">' +
         '<b>' + escHtml(monogramaDeRed(rotulo)) + '</b>' +
         (ico ? '<img src="/' + escHtml(ico) + '.png" alt="" width="96" height="96">' : '') +
         '</span>';
}

function chipRed(nombre, extra){ return chip(nombre, ICONO_DE_RED[nombre], extra); }

// Token images actually served by the facilitator.
const ICONO_DE_TOKEN = {
  usdc: "usdc", usdt: "usdt", eurc: "eurc",
  ausd: "ausd", pyusd: "pyusd", usdg: "usdg",
  rlusd: "rlusd", xrp: null, hbar: null, hts: null
};

// Un token sin PNG NO saca monograma: "US" seria el mismo para usdc, usdt y
// usdg. Devuelve cadena vacia y el simbolo en texto -- que siempre esta al
// lado -- lo dice.
function chipToken(sim, extra){
  const k = String(sim || "").toLowerCase();
  return ICONO_DE_TOKEN[k] ? chip(sim, ICONO_DE_TOKEN[k], extra) : "";
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
    .map(t => String(t?.token).toLowerCase()).filter(t => Object.hasOwn(ICONO_DE_TOKEN, t) && ICONO_DE_TOKEN[t]))].sort();
}

function matchesStablecoinFilter(row, token) {
  return Boolean(row?.schemes.has('exact') && (!token || stablecoinsForCard(row)?.includes(token)));
}

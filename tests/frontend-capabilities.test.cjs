const {test} = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const vm = require('node:vm');
const path = require('node:path');
const root = path.resolve(__dirname, '..');
const context = vm.createContext({});
vm.runInContext(fs.readFileSync(path.join(root, 'static/x402.js'), 'utf8'), context);
const evaluate = code => JSON.parse(JSON.stringify(vm.runInContext(code, context)));
const kind = (network, aliases, tokens, scheme = 'exact') => ({network, networkAliases:aliases, scheme, extra:tokens === undefined ? {} : {tokens:tokens.map(token => ({token}))}});
function catalog(kinds) {
  context.fixture = {kinds};
  return evaluate(`supportedCatalog(fixture).map(r => ({name:r.name, aliases:[...r.aliases].sort(), testnet:r.testnet, stablecoins:stablecoinsForCard(r), tokens:[...r.tokens].sort()}))`);
}

test('XRPL v1/v2 merge and show USDC/RLUSD without treating XRP as a stablecoin', () => {
  const rows = catalog([
    kind('xrpl', ['xrpl','xrpl:0'], ['usdc','rlusd','xrp']),
    kind('xrpl:0', ['xrpl','xrpl:0'], ['usdc','rlusd','xrp']),
    kind('xrpl:1', ['xrpl-testnet','xrpl:1'], ['usdc','rlusd','xrp']),
  ]);
  assert.equal(rows.length, 2);
  assert.deepEqual(rows.map(r => r.stablecoins), [['rlusd','usdc'],['rlusd','usdc']]);
  assert.deepEqual(rows.map(r => r.testnet), [false,true]);
});
test('v2-only Arc and native Hedera survive, including testnets', () => {
  const rows = catalog([
    kind('eip155:5042', ['arc','eip155:5042'], ['usdc','eurc']),
    kind('eip155:5042002', ['arc-testnet','eip155:5042002'], ['usdc','eurc']),
    kind('hedera:mainnet', ['hedera:mainnet'], ['usdc']),
    kind('hedera:testnet', ['hedera:testnet'], ['usdc']),
  ]);
  assert.equal(rows.length,4);
  assert.equal(rows.filter(r => !r.testnet).length,2);
  assert.deepEqual(rows.find(r => r.name==='hedera:mainnet').stablecoins,['usdc']);
});
test('no invented assets for missing metadata, empty lists, unknown networks or schemes', () => {
  assert.equal(catalog([kind('solana',['solana'])])[0].stablecoins,null);
  assert.deepEqual(catalog([kind('sui',['sui'],[])])[0].stablecoins,[]);
  assert.equal(catalog([kind('bsc',['bsc'],['usdc'],'upto')])[0].stablecoins,null);
  assert.deepEqual(catalog([kind('sui',['sui'],['usdc']),kind('sui',['sui'],['ausd'],'escrow')])[0].stablecoins,['usdc']);
  assert.equal(evaluate('stablecoinsForCard(undefined)'),null);
  assert.equal(catalog([kind('custom:network',[],['usdc'])])[0].name,'custom:network');
  assert.throws(() => vm.runInContext('supportedCatalog({error:"unavailable"})',context));
});
test('alias linking is transitive, independent of response ordering', () => {
  const kinds = [kind('one',['one','two'],['usdc']),kind('three',['three','two'],['eurc'])];
  assert.deepEqual(catalog(kinds),catalog([...kinds].reverse()));
  assert.equal(catalog(kinds).length,1);
});
test('all landing cards map to a distinct network, including XRPL testnet', () => {
  const html=fs.readFileSync(path.join(root,'static/index.html'),'utf8');
  const keys=[...html.matchAll(/data-tokens="([^"]+)"/g)].map(m=>m[1]);
  context.keys=keys;
  const names=evaluate('keys.map(landingNetworkName)');
  assert.equal(names.length,new Set(names).size);
  for (const name of ['xrpl','xrpl-testnet','arc','arc-testnet','hedera:mainnet','hedera:testnet']) assert(names.includes(name));
  assert(!html.includes('const TOKEN_SUPPORT'));
});
test('every declared image exists and Arc/Hedera/RLUSD use their supplied images', () => {
  const icons=evaluate('[...new Set([...Object.values(ICONO_DE_RED),...Object.values(ICONO_DE_TOKEN)].filter(Boolean))]');
  icons.forEach(icon=>assert(fs.existsSync(path.join(root,`static/${icon}.png`)),icon));
  assert.deepEqual(evaluate('[ICONO_DE_RED.arc,ICONO_DE_RED["hedera:mainnet"],ICONO_DE_TOKEN.rlusd]'),['arc','hedera','rlusd']);
});

test('single stablecoin filter follows exact capabilities and excludes absent networks', () => {
  context.fixture={kinds:[kind('xrpl',['xrpl','xrpl:0'],['usdc','rlusd','xrp']),kind('bsc',['bsc'],['ausd'])]};
  assert.deepEqual(evaluate('supportedCatalog(fixture).filter(r=>matchesStablecoinFilter(r,"usdc")).map(r=>r.name)'),['xrpl']);
  assert.deepEqual(evaluate('supportedCatalog(fixture).filter(r=>matchesStablecoinFilter(r,"eurc")).map(r=>r.name)'),[]);
  assert.equal(evaluate('supportedCatalog(fixture).filter(r=>matchesStablecoinFilter(r,null)).length'),2);
  assert.equal(evaluate('matchesStablecoinFilter(undefined,null)'),false);
});

test('landing shuffles each grid once on load without an opt-in URL', () => {
  const html=fs.readFileSync(path.join(root,'static/index.html'),'utf8');
  assert(!html.includes('RANDOM_NETWORKS'));
  assert.equal((html.match(/^\s*shuffleNetworkCards\(\);/gm)||[]).length,1);
  const start=html.indexOf('function shuffleNetworkCards()');
  const end=html.indexOf("document.querySelectorAll('[data-token-filter]')",start);
  const code=html.slice(start,end);
  function render(random) {
    const orders=[['a','b','c','d'],['e','f','g']];
    const grids=orders.map(cards=>({querySelectorAll:()=>[...cards],appendChild:card=>{cards.splice(cards.indexOf(card),1);cards.push(card);}}));
    const document={getElementById:id=>({querySelectorAll:()=>[grids[id==='mainnet-tab'?0:1]]})};
    vm.runInNewContext(`${code};shuffleNetworkCards()`,{document,Math:{random,floor:Math.floor}});
    return orders;
  }
  const original=[['a','b','c','d'],['e','f','g']];
  const first=render(()=>0), second=render(()=>0.999);
  assert.notDeepEqual(first,second);
  assert.deepEqual(first.map(cards=>[...cards].sort()),original);
  assert.deepEqual(second.map(cards=>[...cards].sort()),original);
});

// The status dot on each landing card reads GET /health/ready. It shows only
// what the route measured: nothing for ok, unprobed or unreadable, a label
// naming state and reason otherwise.
test('card health: a dot only for degraded or down, labelled with the reason', () => {
  context.body={status:'down',networks:[
    {network:'base',caip2:'eip155:8453',status:'ok',rpc:'ok'},
    {network:'ethereum',caip2:'eip155:1',status:'degraded',reason:'signer_gas_low'},
    {network:'arc',caip2:'eip155:5042',status:'down',reason:'rpc_chain_id_mismatch'},
    {network:'hedera',caip2:'hedera:mainnet',status:'down',reason:'rpc_timeout'},
    {network:'celo-sepolia',caip2:'eip155:11142220',status:'degraded'},
    {network:'polygon',caip2:'eip155:137',status:'unknown-state',reason:'x'},
  ],unchecked:['solana']};
  const show=keys=>{context.keys=keys;return evaluate('keys.map(k=>cardHealth(readinessIndex(body),k))');};
  assert.deepEqual(show(['base-mainnet','solana-mainnet','polygon-mainnet','sui-testnet']),[null,null,null,null]);
  assert.deepEqual(show(['ethereum-mainnet','arc-mainnet','celo-testnet']),[
    {status:'degraded',label:'degraded: signer_gas_low'},
    {status:'down',label:'down: rpc_chain_id_mismatch'},
    {status:'degraded',label:'degraded'},
  ]);
  // Native Hedera is only in /supported under its CAIP-2 id; the card finds it.
  assert.deepEqual(show(['hedera-mainnet']),[{status:'down',label:'down: rpc_timeout'}]);
});
test('card health: an unreadable answer shows nothing, never a red dot for not knowing', () => {
  for (const body of [null,'x',{status:'down',error:'probe_failed'},{error:'rate_limited'},{networks:'no'},{networks:[null,{network:'base'}]}]) {
    context.body=body;
    assert.equal(evaluate('JSON.stringify(cardHealth(readinessIndex(body),"base-mainnet"))'),'null',JSON.stringify(body));
  }
  assert.equal(evaluate('cardHealth(null,"base-mainnet")'),null);
});
test('landing: the dot sits in the lower-left corner, does not move the card and stops blinking on reduced motion', () => {
  const html=fs.readFileSync(path.join(root,'static/index.html'),'utf8');
  const rule=html.slice(html.indexOf('.network-status {'),html.indexOf('}',html.indexOf('.network-status {')));
  for (const decl of ['position: absolute','left: 10px','bottom: 10px','animation: network-status-blink']) assert(rule.includes(decl),decl);
  const reduced=html.slice(html.indexOf('@media (prefers-reduced-motion: reduce)'));
  assert(/^@media \(prefers-reduced-motion: reduce\) \{\s*\.network-status \{\s*animation: none;/.test(reduced));
  const loader=html.slice(html.indexOf('(function loadNetworkStatus()'),html.indexOf('// Curated Bazaar counters.'));
  assert(loader.includes("fetch('/health/ready'"));
  assert(loader.includes('.catch(() => paint(null))'),'a failed read clears the dots');
  assert(loader.includes("setAttribute('aria-label', health.label)"));
  assert(html.includes('<script src="/x402.js?v=20260923"></script>'),'a cached x402.js without cardHealth must not be served to this page');
});

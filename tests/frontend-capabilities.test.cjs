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
test('card health: a dot for down or non-gas degraded, or under 10 settles; none for signer_gas_low above it', () => {
  const signers=(...n)=>n.map((settlesRemaining,index)=>({index,settlesRemaining}));
  context.body={status:'down',networks:[
    {network:'base',caip2:'eip155:8453',status:'ok',signers:signers(9070)},
    // The owner's case: Ethereum mainnet with 24 settles left. No dot.
    {network:'ethereum',caip2:'eip155:1',status:'degraded',reason:'signer_gas_low',signers:signers(24)},
    {network:'arbitrum',caip2:'eip155:42161',status:'degraded',reason:'signer_gas_low'},
    {network:'polygon',caip2:'eip155:137',status:'down',reason:'signer_gas_critical',signers:signers(9)},
    {network:'arc',caip2:'eip155:5042',status:'down',reason:'rpc_chain_id_mismatch',signers:[]},
    {network:'hedera',caip2:'hedera:mainnet',status:'down',reason:'rpc_timeout'},
    {network:'avalanche',caip2:'eip155:43114',status:'degraded',reason:'startup_probe_failed'},
    {network:'celo-sepolia',caip2:'eip155:11142220',status:'degraded'},
    // A published reason of low gas, but one signer is under 10: the signer decides.
    {network:'optimism',caip2:'eip155:10',status:'degraded',reason:'signer_gas_low',signers:signers(500,7)},
    // Any `down` lights it, even with 10+ settles (HEALTH_READY_MIN_SETTLES raised to 20).
    {network:'bsc',caip2:'eip155:56',status:'down',reason:'signer_gas_critical',signers:signers(15)},
    {network:'scroll',caip2:'eip155:534352',status:'unknown-state',reason:'x'},
  ],unchecked:['solana']};
  const show=keys=>{context.keys=keys;return evaluate('keys.map(k=>cardHealth(readinessIndex(body),k))');};
  assert.deepEqual(show(['base-mainnet','ethereum-mainnet','arbitrum-mainnet','solana-mainnet','scroll-mainnet','sui-testnet']),[null,null,null,null,null,null]);
  assert.deepEqual(show(['polygon-mainnet','bsc-mainnet','arc-mainnet','avalanche-mainnet','celo-testnet','optimism-mainnet']),[
    {status:'down',label:'down: signer_gas_critical'},
    {status:'down',label:'down: signer_gas_critical'},
    {status:'down',label:'down: rpc_chain_id_mismatch'},
    {status:'degraded',label:'degraded: startup_probe_failed'},
    {status:'degraded',label:'degraded'},
    {status:'degraded',label:'degraded: signer_gas_low'},
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

// The owner's rule: a supported network never leaves the landing because of its
// health. This runs the landing's own status loader over every real card, through
// a degraded and a down network, an unreadable answer (429 body) and a failed
// fetch, and counts the cards each time. Only the dot may come and go.
test('landing: health never removes or hides a card; the dot follows the owner\'s rule', async () => {
  const html=fs.readFileSync(path.join(root,'static/index.html'),'utf8');
  const loader=html.slice(html.indexOf('(function loadNetworkStatus()'),html.indexOf('// Curated Bazaar counters.'));
  const keys=[...html.matchAll(/data-tokens="([^"]+)"/g)].map(m=>m[1]);
  assert(keys.length>=40 && keys.includes('hedera-mainnet') && keys.includes('ethereum-mainnet'));

  const grid=[];
  for (const key of keys) {
    const card={key,style:{},children:[]};
    card.container={dataset:{tokens:key},closest:sel=>sel==='.network-badge'?card:null};
    card.querySelector=sel=>sel===':scope > .network-status'?card.children.find(c=>c.className==='network-status')||null:null;
    card.append=el=>{el.parent=card;card.children.push(el);};
    card.remove=()=>grid.splice(grid.indexOf(card),1);
    grid.push(card);
  }
  const document={hidden:false,
    querySelectorAll:sel=>sel==='.network-badge [data-tokens]'?grid.map(c=>c.container):[],
    createElement:tag=>{const el={tag,attrs:{},title:'',className:''};
      el.setAttribute=(k,v)=>{el.attrs[k]=String(v);};
      el.remove=()=>el.parent.children.splice(el.parent.children.indexOf(el),1);
      return el;}};
  const ready={status:'down',networks:[
    {network:'base',caip2:'eip155:8453',status:'ok',signers:[{index:0,settlesRemaining:9070}]},
    // 24 settles: low for an operator, not a dot for a visitor.
    {network:'ethereum',caip2:'eip155:1',status:'degraded',reason:'signer_gas_low',signers:[{index:0,settlesRemaining:24}]},
    {network:'polygon',caip2:'eip155:137',status:'down',reason:'signer_gas_critical',signers:[{index:0,settlesRemaining:9}]},
    {network:'avalanche',caip2:'eip155:43114',status:'degraded',reason:'startup_probe_failed',signers:[]},
    {network:'hedera',caip2:'hedera:mainnet',status:'down',reason:'rpc_timeout',signers:[]},
  ]};
  const answers=[
    ()=>Promise.resolve({json:()=>Promise.resolve(ready)}),
    ()=>Promise.resolve({json:()=>Promise.resolve({error:'rate_limited'})}),
    ()=>Promise.reject(new Error('offline')),
    ()=>Promise.resolve({json:()=>Promise.resolve(ready)}),
  ];
  let tick;
  const sandbox={document,window:{},console,
    fetch:()=>answers.shift()(),setInterval:fn=>{tick=fn;},
    readinessIndex:context.readinessIndex,cardHealth:context.cardHealth,
    translations:{en:{'netstatus.degraded':'degraded','netstatus.down':'down'},es:{'netstatus.degraded':'degradada','netstatus.down':'caída'}},
    currentLang:'en'};
  const settle=()=>new Promise(r=>setTimeout(r,5));
  const dots=()=>Object.fromEntries(grid.filter(c=>c.children.length).map(c=>[c.key,c.children.map(d=>d.attrs['aria-label'])]));
  const unchanged=()=>{
    assert.equal(grid.length,keys.length,'a card left the grid');
    assert.deepEqual(grid.map(c=>c.key),keys,'the grid changed order');
    grid.forEach(c=>assert.deepEqual(c.style,{},`${c.key} was restyled`));
  };

  vm.runInNewContext(loader,sandbox);
  await settle();
  unchanged();
  assert.deepEqual(dots(),{'polygon-mainnet':['down: signer_gas_critical'],'avalanche-mainnet':['degraded: startup_probe_failed'],'hedera-mainnet':['down: rpc_timeout']});
  const dot=grid.find(c=>c.key==='hedera-mainnet').children[0];
  assert.equal(dot.title,'down: rpc_timeout');
  assert.equal(dot.attrs.role,'img');

  for (const what of ['an unreadable answer','a failed fetch']) {
    tick(); await settle();
    unchanged();
    assert.deepEqual(dots(),{},`${what} must clear the dots, never add one`);
  }

  tick(); await settle();
  sandbox.currentLang='es';
  sandbox.window.__repaintNetworkStatus();
  unchanged();
  assert.deepEqual(dots(),{'polygon-mainnet':['caída: signer_gas_critical'],'avalanche-mainnet':['degradada: startup_probe_failed'],'hedera-mainnet':['caída: rpc_timeout']});
  assert.equal(answers.length,0);
});
test('landing: the dot label is translated in both dictionaries', () => {
  const html=fs.readFileSync(path.join(root,'static/index.html'),'utf8');
  for (const key of ['netstatus.degraded','netstatus.down']) assert.equal(html.split(`"${key}":`).length-1,2,key);
  assert(html.includes("window.__repaintNetworkStatus?.();"),'a language switch relabels the dots');
});

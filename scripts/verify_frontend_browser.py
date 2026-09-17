from pathlib import Path
from http.server import ThreadingHTTPServer, SimpleHTTPRequestHandler
from threading import Thread
from functools import partial
from urllib.parse import urlparse
from datetime import datetime, timezone
import json, re, copy, os, shutil
from playwright.sync_api import sync_playwright

root=Path(__file__).resolve().parents[1]
(root/'.unused').mkdir(exist_ok=True)
# Offline UI fixture: production aliases plus provider metadata added in this fix.
catalog=json.loads((root/'tests/fixtures/frontend-supported.json').read_text(encoding='utf-8'))
for k in catalog['kinds']:
    if k['scheme']!='exact': continue
    names=k.get('networkAliases',[k['network']])
    plain=next((n for n in names if ':' not in n), k['network'])
    symbol=None
    if plain.startswith(('hedera:','sui','algorand','near','stellar','fogo')): symbol=['usdc']
    if plain=='solana': symbol=['usdc','ausd','pyusd']
    if plain=='solana-devnet': symbol=['usdc','pyusd']
    if plain=='bsc': symbol=['ausd']
    if symbol is not None: k.setdefault('extra',{})['tokens']=[{'token':t,'address':'ui-fixture','decimals':7 if plain.startswith('stellar') else 6} for t in symbol]
keys=re.findall(r'data-balance="([^"]+)"',(root/'static/index.html').read_text(encoding='utf-8'))
balances={'balances':{key:0 for key in keys}}
state={'catalog':catalog,'fail':False,'balancesFail':False}
class Handler(SimpleHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_GET(self):
        path=urlparse(self.path).path
        payload=None; status=200
        if path=='/supported': payload=state['catalog']; status=503 if state['fail'] else 200
        elif path=='/api/balances': payload=balances; status=503 if state['balancesFail'] else 200
        elif path in ['/dx402/stats','/discovery/stats','/health','/version']: payload={}
        elif path in ['/networks','/']: self.path='/networks.html' if path=='/networks' else '/index.html'
        if payload is not None:
            content=json.dumps(payload).encode();self.send_response(status);self.send_header('Content-Type','application/json');self.send_header('Content-Length',str(len(content)));self.end_headers();self.wfile.write(content);return
        super().do_GET()
server=ThreadingHTTPServer(('127.0.0.1',0),partial(Handler,directory=str(root/'static')))
Thread(target=server.serve_forever,daemon=True).start()
base=f'http://127.0.0.1:{server.server_port}'
report={'checkedAt':datetime.now(timezone.utc).isoformat(),'mode':'offline browser fixtures; no payment/RPC calls','checks':[]}
with sync_playwright() as p:
    chrome=os.getenv('CHROME_EXECUTABLE') or shutil.which('google-chrome') or shutil.which('chromium')
    windows_chrome=Path('C:/Program Files/Google/Chrome/Application/chrome.exe')
    if not chrome and windows_chrome.is_file(): chrome=str(windows_chrome)
    browser=p.chromium.launch(**({'executable_path':chrome} if chrome else {}),headless=True)
    context=browser.new_context(viewport={'width':1440,'height':1000})
    context.route('**/*',lambda route: route.continue_() if route.request.url.startswith(base) else route.abort())
    page=context.new_page(); errors=[];page.on('pageerror',lambda e:errors.append(str(e)))
    page.goto(base);page.wait_for_function("window.__catalogState==='ready'")
    expected={'xrpl-mainnet':['RLUSD','USDC'],'xrpl-testnet':['RLUSD','USDC'],'arc-mainnet':['EURC','USDC'],'arc-testnet':['EURC','USDC'],'hedera-mainnet':['USDC'],'hedera-testnet':['USDC'],'sui-mainnet':['USDC'],'bsc-mainnet':['AUSD']}
    for key,tokens in expected.items():
        actual=page.locator(f'[data-tokens="{key}"] img').evaluate_all('(els)=>els.map(e=>e.alt).sort()')
        assert actual==tokens,(key,actual)
    assert page.locator('[data-balance="xrpl-mainnet"]').inner_text()=='0'
    assert page.locator('.network-badge:visible').count()==23
    page.locator('[data-tokens="xrpl-mainnet"]').locator('..').screenshot(path=str(root/'.unused/xrpl-card-local.png'))
    visible=lambda: sorted(page.locator('.network-badge:visible [data-tokens]').evaluate_all('(els)=>els.map(e=>e.dataset.tokens)'))
    page.locator('[data-token-filter="usdc"]').click()
    assert len(visible())==21,visible()
    assert 'bsc-mainnet' not in visible() and 'robinhood-mainnet' not in visible()
    page.locator('[data-token-filter="eurc"]').click()
    assert visible()==sorted(['ethereum-mainnet','base-mainnet','avalanche-mainnet','arc-mainnet']),visible()
    page.locator('[data-token-filter="rlusd"]').focus();page.keyboard.press('Enter')
    assert visible()==['xrpl-mainnet']
    page.locator('button[onclick*="testnet"]').click()
    assert visible()==['xrpl-testnet']
    page.locator('[data-token-filter="usdt"]').click()
    assert not visible()
    assert 'No networks support USDT' in page.locator('#stablecoin-filter-status').text_content(),(page.locator('#stablecoin-filter-status').text_content(),errors,page.evaluate("[...document.querySelectorAll('.tab-content.active .network-badge')].map(c=>[c.className,c.querySelector('[data-tokens]')?.dataset.tokens])"))
    page.locator('[data-token-filter=""]').click()
    page.locator('button[onclick*="mainnet"]').click()
    assert len(visible())==23
    page.locator('[data-token-filter="usdc"]').click();page.locator('[data-token-filter="usdc"]').click()
    assert len(visible())==23
    sizes=page.locator('.token-logo:visible').evaluate_all('(els)=>els.map(e=>({w:e.getBoundingClientRect().width,h:e.getBoundingClientRect().height}))')
    assert all(abs(x['w']-32)<0.1 and abs(x['h']-32)<0.1 for x in sizes),sizes
    page.locator('.stablecoin-bar').screenshot(path=str(root/'.unused/stablecoin-filters-local.png'))
    report['checks']+=['USDC and EURC exact network sets','RLUSD keyboard filter and persistence across tabs','Empty testnet result','Clear and toggle filter','32px visible logo frames in filters and cards']
    page.locator('.lang [data-lang="es"]').click()
    assert page.locator('[data-tokens="xrpl-mainnet"] .tokens-label').text_content()=='Monedas estables admitidas'
    for width in [1440,390]:
        page.set_viewport_size({'width':width,'height':1000})
        assert page.evaluate('document.documentElement.scrollWidth===innerWidth'),width
    page.locator('[data-token-filter="rlusd"]').click()
    assert '1 de 23 redes' in page.locator('#stablecoin-filter-status').inner_text()
    page.locator('[data-token-filter=""]').click()
    page.locator('button[onclick*="testnet"]').click()
    assert page.locator('[data-tokens="xrpl-testnet"]').is_visible()
    assert page.locator('.wallet-addresses [data-native-account="hedera:mainnet"]').inner_text().strip()=='0.0.10868300'
    report['checks']+=['All affected stablecoin badges','Numeric zero balance','23 mainnet cards','EN/ES badges','Desktop/mobile overflow','XRPL testnet tab','Hedera wallet ID']
    page.set_viewport_size({'width':1440,'height':1000});page.goto(base+'/networks')
    page.wait_for_function("document.querySelectorAll('#rows tr').length===23")
    for name in ['xrpl','sui','hedera:mainnet','arc']:
        row=page.locator('#rows tr').filter(has=page.locator(f'.chip-red[title="{name}"]'))
        assert row.count()==1,name
        assert 'USDC' in row.inner_text(),row.inner_text()
    assert page.locator('#rows .chip-red[title="arc"] img').get_attribute('src')=='/arc.png'
    assert page.locator('#rows .chip-red[title="hedera:mainnet"] img').get_attribute('src')=='/hedera.png'
    page.locator('#live [data-filter="testnet"]').click()
    assert page.locator('#rows .chip-red[title="xrpl-testnet"]').count()==1
    page.set_viewport_size({'width':390,'height':844})
    assert page.evaluate('document.documentElement.scrollWidth===innerWidth')
    report['checks']+=['Networks table tokens and icons','Networks table XRPL testnet','Networks mobile overflow']
    # A deployment with v2 only must keep the same known rows/cards.
    state['catalog']=copy.deepcopy(catalog)
    state['catalog']['kinds']=[k for k in catalog['kinds'] if k['network'].count(':')]
    page.goto(base);page.wait_for_function("window.__catalogState==='ready'")
    assert page.locator('.network-badge:visible').count()==23
    assert page.locator('[data-tokens="xrpl-mainnet"] img').count()==2
    report['checks'].append('CAIP-2-only catalog retains alias-linked cards')
    # Never claim token support when discovery fails, and never leave loading
    # forever on a network with no RPC fallback (Hedera).
    state['fail']=True;state['balancesFail']=True
    page.goto(base);page.wait_for_function("window.__catalogState==='unavailable'")
    assert page.locator('[data-tokens] img').count()==0
    page.wait_for_function("document.querySelector('[data-balance=hedera-mainnet]').textContent==='N/A'")
    report['checks'].append('Catalog/balance outage degrades without invented assets or stuck Hedera balance')
    assert not errors,errors
    report['pageErrors']=errors
    browser.close()
server.shutdown()
(root/'.unused/frontend-browser-local.json').write_text(json.dumps(report,indent=2)+'\n',encoding='utf-8')
print(json.dumps(report,indent=2))

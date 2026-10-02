#!/usr/bin/env python3
"""Write the Bazaar search benchmark: a 1 999-listing catalog and twelve intents.

Outputs, both under tests/fixtures/bazaar/:

  search-catalog.json  the listings, one compact row each
  search-intents.json  the intents, the URLs that do each job, and the keywords

Deterministic: running it twice writes the same bytes. It reads nothing and calls
nothing. The rows are compact so any consumer of the Bazaar (Paarce in Emporium,
KarmaKadabra's bazaar client, this facilitator's own test) can load the same set:

  {"url": ..., "description": ..., "type": "mcp", "category": ..., "tags": [...],
   "provider": ..., "method": "POST", "inputFields": [...], "pending": true}

Absent keys are empty. A row's position is its age: the first row is the newest.

`pending: true` marks a listing the public surface does not show: not verified alive (a 402 to its
declared method, fresh, not quarantined). Modelled, not measured, and stated here so nobody reads it
as data: every service the partner found answering 402 is exposed, and so are the paid essays (they
charge); the quote API's 86 templated endpoints are pending (the partner found the family quarantined
and only /stock-quote live); 40 % of the long tail is pending. A benchmark over the exposed rows is
what a router querying the curated bazaar sees.

The catalog has the shape a router partner measured on the live catalog on
2026-10-01 (1 999 listings, four hosts holding 1 043, about 40 % of the long tail
without a description), and for each intent three or four services that do the
job next to listings that only share its words. Where the live catalog has no such
service (Reddit search, an email finder), the fixture adds them: the benchmark
measures search, not coverage. "Does the job" is strict: an email validator does
not find an email, a WHOIS record is not a company record, an essay about image
generation does not generate an image.

Usage: python3 scripts/bazaar_search_fixture.py
"""

import json
import pathlib

ROOT = pathlib.Path(__file__).resolve().parent.parent
OUT = ROOT / "tests" / "fixtures" / "bazaar"

INTENTS = [
    ("work-email", "Find a person's work email", ["email", "email finder", "work email"]),
    ("company-record", "Company record by domain", ["company", "domain", "enrich"]),
    ("who-works", "Who works at a company", ["employees", "people", "company"]),
    ("web-search", "Web search", ["search", "web search", "serp"]),
    ("read-page", "Read a page", ["scrape", "read", "page"]),
    ("crypto-price", "Crypto price", ["price", "crypto", "token price"]),
    ("weather", "Weather", ["weather", "forecast"]),
    ("x-search", "X / Twitter search", ["twitter", "tweets", "twitter/search"]),
    ("reddit-search", "Reddit search", ["reddit", "subreddit"]),
    ("phone-lookup", "Phone number lookup", ["phone", "phone-lookup", "carrier"]),
    ("stock-quote", "Stock quote", ["stock", "quote", "stock-quote"]),
    ("image-generation", "Image generation", ["image", "generate", "text-to-image"]),
]

# Requests the lexicon was not written against, in both languages.
HELD_OUT = [
    ("get the current bitcoin price in dollars", "crypto-price"),
    ("scrape the text of a webpage", "read-page"),
    ("what's the temperature in Bogotá tomorrow", "weather"),
    ("search tweets about a topic", "x-search"),
    ("generate a picture from a prompt", "image-generation"),
    ("precio de una acción en la bolsa", "stock-quote"),
    ("buscar en la web", "web-search"),
    ("el clima en Medellín", "weather"),
    ("encontrar el correo de trabajo de una persona", "work-email"),
    ("buscar publicaciones en reddit", "reddit-search"),
]


def row(url, description="", job=None, **extra):
    r = {"url": url}
    if description:
        r["description"] = description
    r.update({k: v for k, v in extra.items() if v})
    return r, job


CORE = [
    # Find a person's work email
    row("https://api.mailfinder.example/v1/find",
        "Find the verified work email address of a person from their full name and company domain.",
        "work-email", category="people", tags=["email", "b2b"], method="POST",
        inputFields=["full_name", "company_domain"]),
    row("https://stableenrich.dev/api/hunter/email-finder",
        "Email finder: returns the professional email address for a first name, last name and company domain, with a confidence score.",
        "work-email", method="GET", inputFields=["first_name", "last_name", "domain"]),
    row("https://contactout.x402.example/person/email",
        "Look up a contact's business email and LinkedIn profile from their name and employer.", "work-email"),
    row("https://email-auth.use.x402atlas.com/check", "Check SPF, DKIM and DMARC records for an email sending domain."),
    row("https://mailer.x402.example/send", "Send a transactional email to any address."),
    row("https://verify.x402.example/email", "Validate whether an email address is deliverable (syntax, MX, SMTP)."),
    # Company record by domain
    row("https://stableenrich.dev/api/company/enrich",
        "Company enrichment by domain: legal name, industry, headcount, funding and headquarters.",
        "company-record", method="GET", inputFields=["domain"]),
    row("https://api.firmographics.example/v1/company",
        "Firmographic profile of an organization given its website domain.", "company-record"),
    row("https://companydata.x402.example/lookup",
        "Company record lookup: registration details, founding year and social profiles for a business domain.",
        "company-record"),
    row("https://dns.use.x402atlas.com/whois", "WHOIS record for a domain: registrar, creation date and nameservers."),
    row("https://news.x402.example/company", "Latest news headlines that mention a company."),
    row("https://domains.x402.example/available", "Check whether a domain name is available to register."),
    # Who works at a company
    row("https://stableenrich.dev/api/fullenrich/people-search",
        "People search: find employees at a company by domain, job title and seniority.",
        "who-works", method="POST",
        inputFields=["current_company_domains", "current_position_titles", "person_names"]),
    row("https://api.orgchart.example/v1/employees",
        "List the employees of a company with name, title and seniority.", "who-works"),
    row("https://teamlookup.x402.example/staff",
        "Staff directory: the people working at an organization, by company domain.", "who-works"),
    row("https://jobs.x402.example/postings", "Open job postings for a company."),
    row("https://api.salaries.example/v1/compensation", "Salary ranges by role and company."),
    # Web search
    row("https://x402.tavily.com/search",
        "Tavily web search for AI agents: ranked results with snippets and sources.",
        "web-search", method="POST", inputFields=["query", "max_results", "search_depth"]),
    row("https://api.exa.x402.example/search",
        "Neural search over the open web; returns URLs and highlights.", "web-search"),
    row("https://serp.x402.example/google",
        "Google search results (SERP) for a query: organic results, news and related questions.", "web-search"),
    row("https://search.brave.x402.example/web",
        "Search the internet and return the top results with titles and snippets.", "web-search"),
    row("https://polymarket.x402.example/markets/search", "Search Polymarket prediction markets by keyword."),
    row("https://nft.x402.example/collections/search", "Search NFT collections by name."),
    # Read a page
    row("https://api.firecrawl.x402.example/scrape",
        "Scrape any URL and return the page content as clean markdown.",
        "read-page", method="POST", inputFields=["url", "formats"]),
    row("https://reader.x402.example/read",
        "Read a web page: fetches the URL and extracts the main article text.", "read-page"),
    row("https://extract.x402.example/v1/extract", "Extract the text, title and links of an HTML page.", "read-page"),
    row("https://screenshotone.x402.paysponge.com/take"),
    row("https://pdf.x402.example/convert", "Convert a PDF document to text."),
    # Crypto price
    row("https://api.coinprice.x402.example/v1/price",
        "Real-time cryptocurrency price for a symbol (BTC, ETH, SOL) in USD.",
        "crypto-price", method="GET", inputFields=["symbol"]),
    row("https://api.onesource.io/api/chain/token-price",
        "Current USD price of an ERC20 token from on-chain DEX liquidity.",
        "crypto-price", method="GET", inputFields=["contract", "network"]),
    row("https://quotes.cryptodata.example/spot", "Spot quote for a crypto asset across major exchanges.", "crypto-price"),
    row("https://api.onesource.io/api/chain/erc20-balance",
        "ERC20 token balance for any Ethereum wallet - USDC, USDT, DAI, or any token - via balanceOf (eth_call) on OneSource live Ethereum RPC",
        None, method="GET", inputFields=["address", "contract", "network"]),
    row("https://x402.minara.ai/x402/perp-trading-suggestion", "Perp trading suggestion for a crypto pair."),
    row("https://nft.x402.example/floor", "NFT collection floor price."),
    # Weather
    row("https://weather.x402.example/current",
        "Current weather conditions for a city or coordinates: temperature, wind, humidity.",
        "weather", method="GET", inputFields=["city", "lat", "lon"]),
    row("https://api.forecast.example/v1/forecast",
        "Seven-day forecast with hourly temperature and precipitation.", "weather"),
    row("https://meteo.x402.example/now", "Live meteorological observations by location.", "weather"),
    row("https://climate.x402.example/risk", "Climate risk score (flood, wildfire) for a property address."),
    # X / Twitter search
    row("https://glim.sh/api/v1/twitter/search",
        "Search recent tweets on X by keyword, with author and engagement counts.",
        "x-search", method="POST", inputFields=["query", "limit"]),
    row("https://api.vibe.airforce/api/x402/vibe-tools/twitter/search", "", "x-search"),
    row("https://xsearch.x402.example/tweets", "Search posts on X (formerly Twitter) that match a query.", "x-search"),
    row("https://glim.sh/api/v1/twitter/user", "Profile of an X account: bio, followers and recent tweets."),
    row("https://social.x402.example/sentiment", "Sentiment score of a piece of social media text."),
    # Reddit search
    row("https://reddit.x402.example/search", "Search Reddit posts and comments across subreddits.", "reddit-search"),
    row("https://api.socialscan.example/reddit",
        "Reddit search API: find threads by keyword and subreddit.", "reddit-search"),
    row("https://subreddit.x402.example/v1/posts",
        "Search subreddit submissions by query and time range.", "reddit-search"),
    row("https://hn.x402.example/search", "Search Hacker News stories and comments."),
    # Phone number lookup
    row("https://tenjin.sh/api/phone-lookup",
        "Phone number lookup: carrier, line type and owner name for a number.",
        "phone-lookup", method="POST", inputFields=["phone"]),
    row("https://numverify.x402.example/validate",
        "Validate a phone number and return its country, carrier and line type.", "phone-lookup"),
    row("https://callerid.x402.example/lookup", "Reverse lookup: who owns this mobile number.", "phone-lookup"),
    row("https://sms.x402.example/send", "Send an SMS message to a phone number."),
    row("https://proxy.x402.example/mobile", "Rotating mobile proxy IPs by country."),
    # Stock quote (the third is api.losbeto.xyz/stock-quote, in its family below)
    row("https://markets.x402.example/v1/quote",
        "Delayed equity quote (NASDAQ, NYSE) for a ticker symbol.", "stock-quote"),
    row("https://finance.x402.example/stocks/price",
        "Latest share price and daily range for a listed company.", "stock-quote"),
    row("https://fx.x402.example/rate", "Foreign exchange rate between two currencies."),
    # Image generation
    row("https://imagegen.x402.example/v1/generate",
        "Generate an image from a text prompt (text-to-image).",
        "image-generation", method="POST", inputFields=["prompt", "size"]),
    row("https://art.x402.example/render",
        "AI art generator: create pictures from a description.", "image-generation"),
    row("https://flux.x402.example/txt2img",
        "Text to image with FLUX models; returns a PNG URL.", "image-generation"),
    row("https://clawhunter.fun/api/v1/tools/image-prompts"),
    row("https://vision.x402.example/moderate", "Classify an image as safe or unsafe."),
    row("https://bg.x402.example/remove", "Remove the background of an image."),
    # First-party listings (tiered by config/bazaar_curation.json).
    row("https://api.describe.net/reputation/wallet/0xabc", "Reputation score of a wallet."),
    row("https://mcp.execution.market/mcp",
        "Execution Market MCP: post tasks for human workers and pay them on completion.", type="mcp"),
    row("https://api.402milly.xyz/purchase", "Buy a pixel on the million dollar homepage."),
]

ESSAYS_ON_TOPIC = [
    "Why work email is broken, and what replaces it",
    "The search for a better search engine",
    "How I read a page a day for a year",
    "Crypto prices are a story we tell ourselves",
    "Weather apps lie about rain",
    "What Twitter search taught me about attention",
    "Reddit as the last honest search engine",
    "The phone number is the new identity",
    "Stock quotes and the illusion of control",
    "Image generation and the death of stock photos",
    "Who really works at a startup",
    "Every company record tells a founding myth",
]
ESSAY_WORDS = [
    ["quiet", "slow", "honest", "hidden", "strange", "small", "long", "late", "open", "missing",
     "patient", "careful"],
    ["agents", "payments", "latency", "writing", "focus", "memory", "design", "compilers", "teams",
     "habits", "markets", "craft", "attention", "trust", "protocols"],
    ["software", "money", "cities", "language", "learning", "machines", "work", "time", "taste",
     "risk", "scale", "music"],
]
# The data-pack host's paths have the live catalog's shape (2026-10-01): /x402/demand-<words>, no
# description, the words themselves naming a market, a company or an API concern.
PACK_KINDS = ["company", "market", "crypto", "macro"]
PACK_SUBJECTS = ["oracle", "lly", "nvidia", "apple", "tesla", "stripe", "avalanche", "bitcoin",
                 "solana", "ethereum", "gold", "oil", "euro", "yen", "copper", "wheat", "lithium",
                 "uranium", "shipping", "housing", "jobs", "inflation", "rates", "retail", "energy"]
PACK_MEASURES = ["revenue", "gross-profit", "ticker", "volume"]
TICKERS = ["AAPL", "MSFT", "NVDA", "TSLA", "AMZN", "GOOG", "META", "NFLX", "AMD", "INTC", "ORCL",
           "IBM", "UBER", "SHOP", "COIN", "PYPL", "DIS", "NKE", "KO", "PEP", "WMT", "JPM", "BAC",
           "MA", "XOM", "CVX", "PFE", "MRK", "CSCO"]
ARCHETYPES = [
    ("translate", "/v1/translate", "Translate text between languages."),
    ("ocr", "/ocr", "Optical character recognition: text from a scanned image."),
    ("sentiment", "/sentiment", "Sentiment analysis of a text."),
    ("summarize", "/summarize", "Summarize a long document into bullet points."),
    ("geocode", "/geocode", "Geocode an address to latitude and longitude."),
    ("fx", "/convert", "Currency conversion at the latest exchange rate."),
    ("flight", "/flights/status", "Flight status by flight number."),
    ("sports", "/odds", "Live sports odds and scores."),
    ("news", "/news", "Top news headlines by topic."),
    ("dns", "/dns", "DNS lookup for a hostname."),
    ("ipgeo", "/ip", "IP address geolocation."),
    ("nftmeta", "/nft/metadata", "NFT metadata for a token id."),
    ("labels", "/wallet/labels", "Labels and risk flags for a wallet address."),
    ("gas", "/gas", "Gas price oracle for EVM chains."),
    ("txdecode", "/tx/decode", "Decode an EVM transaction."),
    ("llm", "/chat", "Chat completion with an open model."),
    ("embed", "/embed", "Text embeddings for semantic search."),
    ("tts", "/tts", "Text to speech audio."),
    ("transcribe", "/transcribe", "Transcribe audio to text."),
    ("qr", "/qr", "Generate a QR code for a string."),
    ("pdfgen", "/pdf", "Generate a PDF invoice."),
    ("review", "/review", "Automated code review for a diff."),
    ("domainage", "/domain/age", "Age of a domain in days."),
    ("yields", "/yields", "DeFi yield opportunities by chain."),
    ("bridge", "/bridge/quote", "Bridge quote between chains."),
    ("swap", "/swap/quote", "Swap quote from a DEX aggregator."),
    ("predict", "/markets", "Prediction market odds."),
    ("horoscope", "/horoscope", "Daily horoscope."),
    ("recipes", "/recipes", "Recipe suggestions from ingredients."),
    ("holidays", "/holidays", "Public holidays by country."),
    ("timezone", "/time", "Current time in a timezone."),
    ("math", "/math", "Solve a math expression."),
    ("random", "/random", "Verifiable random numbers."),
    ("lyrics", "/lyrics", "Song lyrics by title."),
    ("movies", "/movies", "Movie details by title."),
    ("books", "/books", "Book details by ISBN."),
    ("trademark", "/trademarks/search", "Trademark search by name."),
    ("patent", "/patents/search", "Patent search by keyword."),
    ("jobs", "/jobs", "Job listings by city."),
    ("property", "/property/value", "Property value estimate for an address."),
]


def pseudo_address(seed):
    """A deterministic 0x-address for templated paths (xorshift64)."""
    x = 0x9E3779B97F4A7C15 ^ seed
    mask = (1 << 64) - 1
    out = ""
    for _ in range(3):
        x ^= (x << 13) & mask
        x ^= x >> 7
        x ^= (x << 17) & mask
        out += f"{x:016x}"
    return "0x" + out[:40]


def catalog():
    rows = list(CORE)

    # api.losbeto.xyz: 87 listings without descriptions; one does the job.
    rows.append(row("https://api.losbeto.xyz/stock-quote", "", "stock-quote"))
    losbeto = []
    for endpoint in ["stock-history", "options-chain", "earnings"]:
        for ticker in TICKERS:
            losbeto.append(row(f"https://api.losbeto.xyz/{endpoint}/{ticker}", pending=True))
    rows += losbeto[:86]

    # tenjin.blog: 379 paid essays, VIP through config/bazaar_curation.json.
    essays = list(ESSAYS_ON_TOPIC)
    for a in ESSAY_WORDS[0]:
        for b in ESSAY_WORDS[1]:
            for c in ESSAY_WORDS[2]:
                if len(essays) < 379:
                    essays.append(f"The {a} {b} of {c}")
    for title in essays:
        slug = "".join(ch if ch.isalnum() and ch.isascii() else "-" for ch in title.lower())
        rows.append(row(f"https://tenjin.blog/api/read/{slug}", f"Paid essay on tenjin.blog: {title}",
                        provider="Tenjin"))

    # market.datapackvibe.com: 388 data packs without descriptions, one template.
    packs = [f"demand-{k}-{s}-{m}" for k in PACK_KINDS for s in PACK_SUBJECTS for m in PACK_MEASURES]
    for slug in packs[:388]:
        rows.append(row(f"https://market.datapackvibe.com/x402/{slug}"))

    # mpp.hyreagent.fun: 189 token-analytics endpoints without descriptions.
    rows.append(row("https://mpp.hyreagent.fun/base/debridge/quote"))
    shapes = ["/trenches/token/{a}/snipers", "/traders/token/{a}/whales", "/trenches/token/{a}/verdict",
              "/base/trenches/token/{a}"]
    for i in range(188):
        rows.append(row("https://mpp.hyreagent.fun" + shapes[i % len(shapes)].replace("{a}", pseudo_address(i))))

    # The long tail: 40 archetypes x 22-23 (892, to reach the 1 999 measured),
    # about three listings a host, 40 % undescribed.
    i = 0
    for index, (name, path, description) in enumerate(ARCHETYPES):
        for k in range(23 if index < 12 else 22):
            prefix = ["/v1", "/v2", "/api"][k % 3]
            rows.append(row(f"https://{name}{k // 3}.x402.example{prefix}{path}",
                            "" if i % 5 < 2 else description, pending=i % 5 in (0, 3)))
            i += 1
    return rows


def main():
    rows = catalog()
    listings = [r for r, _ in rows]
    urls = [r["url"] for r in listings]
    assert len(urls) == len(set(urls)) == 1999, len(urls)
    expected = {}
    for r, job in rows:
        if job:
            expected.setdefault(job, []).append(r["url"])
    intents = []
    for intent_id, text, keywords in INTENTS:
        assert len(expected[intent_id]) >= 3, intent_id
        intents.append({"id": intent_id, "text": text, "keywords": keywords,
                        "expected": expected[intent_id]})
    doc = {
        "$comment": ("Twelve agent intents as a router sends them, the catalog URLs that do each job, "
                     "and the keywords a person would try instead. Catalog: search-catalog.json. "
                     "Generated by scripts/bazaar_search_fixture.py; do not edit by hand."),
        "catalog": "search-catalog.json",
        "intents": intents,
        "heldOut": [{"text": t, "intent": i} for t, i in HELD_OUT],
    }
    OUT.mkdir(parents=True, exist_ok=True)
    # One listing per line, so a diff of the fixture reads as a diff of listings.
    lines = [json.dumps(r, ensure_ascii=False, separators=(",", ":")) for r in listings]
    (OUT / "search-catalog.json").write_text(
        "[\n" + ",\n".join(lines) + "\n]\n", encoding="utf-8", newline="\n")
    (OUT / "search-intents.json").write_text(
        json.dumps(doc, ensure_ascii=False, indent=2) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(listings)} listings, {len(intents)} intents, {len(HELD_OUT)} held-out requests")


if __name__ == "__main__":
    main()

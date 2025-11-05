# Landing Page Setup

## 🎉 What You Have

A clean, minimal, monochrome landing page for your x402 facilitator:

- ✅ **Bilingual** (English/Spanish)
- ✅ **Auto dark mode** (follows system preference)
- ✅ **Information-dense** (similar to facilitator.x402.rs)
- ✅ **Zero dependencies** (no external CDNs or fonts)
- ✅ **Responsive design** (mobile, tablet, desktop)

## 📍 Where to Place Your Logo

### Option 1: Replace the Placeholder (Recommended)

Replace the 1x1 transparent placeholder with your actual logo:

```bash
cd x402-rs/static
cp /path/to/ultravioletadao.png logo.png
```

**Logo specs:**
- Format: PNG (transparent background recommended)
- Size: 48×48px displayed (provide 96×96px for retina)
- Location: Shows in **top left header**, next to "x402 Facilitator" text

### Option 2: Keep the Placeholder

The page works fine without a logo. The placeholder is invisible (1×1 transparent PNG), so nothing shows if you don't replace it.

## 🔨 Building

Once you've added your logo (or not), build the facilitator:

```bash
. "$HOME/.cargo/env"  # Load cargo if needed
cd x402-rs
cargo build --release
```

This embeds:
- The HTML landing page
- Your logo (or placeholder)
- Everything into a single binary

## 🚀 Running

```bash
cargo run --release
```

Then open: `http://localhost:8080/`

## 🧪 Testing

**Test checklist:**
- [ ] Page loads at http://localhost:8080/
- [ ] Logo appears in top left (if you added one)
- [ ] Language switcher works (EN ↔ ES buttons top right)
- [ ] All sections display correctly
- [ ] Code example is readable
- [ ] Chains grid is responsive
- [ ] Dark mode works (if your OS is in dark mode)
- [ ] Mobile view looks good (resize browser)

## 🎨 Customization

### Update the Facilitator URL

Edit `static/index.html`, find line ~428:

```javascript
url: 'https://facilitator.ultravioletadao.xyz'
```

Change to your actual domain.

### Add More Chains

Edit `static/index.html`, find the chains section (~436):

```html
<div class="chains">
    <div class="chain">Your Chain Name</div>
    <!-- Add more -->
</div>
```

### Change Footer Links

Edit `static/index.html`, find footer (~520):

```html
<div class="footer-links">
    <a href="https://x402.org">Protocol</a>
    <a href="https://github.com/...">GitHub</a>
    <a href="/health">Status</a>
</div>
```

## 📁 Files

```
x402-rs/
├── static/
│   ├── index.html       ← Landing page (27KB)
│   ├── logo.png         ← YOUR LOGO GOES HERE (replace me!)
│   ├── README.md        ← This file
│   └── SETUP.md         ← Setup instructions
├── src/
│   ├── handlers.rs      ← Added get_index() and get_logo()
│   └── main.rs          ← Added routes: / and /logo.png
└── ...
```

## 🌐 Production Deployment

Once deployed to `facilitator.ultravioletadao.xyz`:

**Landing page:**
https://facilitator.ultravioletadao.xyz/

**API endpoints** (unchanged):
- https://facilitator.ultravioletadao.xyz/health
- https://facilitator.ultravioletadao.xyz/supported
- https://facilitator.ultravioletadao.xyz/verify
- https://facilitator.ultravioletadao.xyz/settle

## 💡 Design Philosophy

**Inspired by facilitator.x402.rs, but cooler:**

- Pure monochrome (no brand colors)
- Cleaner typography
- Better spacing
- Auto dark mode
- Bilingual support
- More information-dense

## 🔍 Logo Visibility

The logo shows **only if it's a valid PNG** and larger than 1×1 pixels.

The placeholder (1×1 transparent PNG) is invisible, so if you don't replace it, no logo appears—which is fine!

## ❓ Questions?

See the full documentation in `LANDING_PAGE.md`.

---

**Ready to build?**

```bash
# 1. Add your logo (optional)
cp ~/ultravioletadao.png x402-rs/static/logo.png

# 2. Build
cd x402-rs
cargo build --release

# 3. Run
cargo run --release

# 4. Open browser
open http://localhost:8080
```

Enjoy your new landing page! 🎉

# Static Assets

This directory contains assets embedded into the facilitator binary at compile time.

## Files

- **index.html** - Landing page (bilingual: English/Spanish)
- **logo.png** - Ultravioleta DAO logo (48×48px, PNG format)

## Adding Your Logo

Replace the placeholder `logo.png` with your actual logo:

```bash
cp /path/to/your/ultravioletadao.png static/logo.png
```

Then rebuild:

```bash
cargo build --release
```

The logo will be embedded in the binary and served at `/logo.png`.

If no logo is present, the build will use a transparent placeholder.

# Falcon DM browser extension (MV3)

Load unpacked from this folder in Chrome/Edge.

## API notes

- Pair: `POST /api/pair` → `200` + token, or `202` pending (approve in Falcon Settings). Extension polls until approved.
- Downloads: `POST /api/intercept` or `/api/add` with `X-Falcon-Token`.
- YouTube quality: send JSON field `format` (yt-dlp `-f` selector). Do **not** put format in the URL. Legacy `#falconfmt=` still accepted server-side as internal storage.
- Origin must be `chrome-extension://<id>` and that id must be allowlisted after Settings approve.

## YouTube

Desktop app needs `yt-dlp` on PATH (or set path in Falcon Settings). Extension sends watch URL + `format`; never googlevideo CDN URLs.

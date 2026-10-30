#!/usr/bin/env bash

#MISE description="Shared: fixture FMP server + seeded heydayd for the app's NSH tests"
#MISE private=true

# The app's headless and host tests boot the production root, which now
# connects through the gate. This script serves fmp-rs's recorded payloads
# over HTTP (FMP endpoint paths), derives per-symbol copies of the recorded
# quote/profile/chart/ratios for the seeded watchlist, starts heydayd
# against them with an already-migrated, seeded sqlite store, and waits for
# /healthz. Call `server_start`; `server_stop` runs on EXIT.

proj_dir=$(git rev-parse --show-toplevel)
cd "$proj_dir" || exit 1

FIXTURES="crates/heydayd/src/market/fixtures"
WORK="target/test-server"
# 17810, not heydayd's product default 17800: the stack must never
# collide with (or hijack connections meant for) a real running heydayd.
ADDR="127.0.0.1:17810"
FMP_PORT=17999

server_start() {
	mkdir -p "$WORK/root/news" "$WORK/root/insider-trading" "$WORK/root/historical-price-eod"

	# Derive per-symbol recordings: the AAPL recording, its symbol string
	# (and the quote's price where a number must differ) replaced per
	# symbol. Shapes are the recordings', byte for byte, apart from that.
	python3 - "$FIXTURES" "$WORK/root" <<'PY'
import json, sys, pathlib

src, root = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2])
SYMBOLS = {"MSFT": 415.25, "NVDA": 132.40, "TSM": 188.10, "GEV": 512.70}

quote = json.loads((src / "quote.json").read_bytes())[0]
profile = json.loads((src / "company_profile.json").read_bytes())[0]
chart = json.loads((src / "chart_full.json").read_bytes())
ratios = json.loads((src / "ratios.json").read_bytes())
insider = json.loads((src / "insider_trading.json").read_bytes())

quotes = [quote]
profiles = [profile]
charts = {q["symbol"]: chart for q in quotes}
ratios_by = {q["symbol"]: ratios for q in quotes}
insiders = {q["symbol"]: insider for q in quotes}
for symbol, price in SYMBOLS.items():
    q = dict(quote)
    q["symbol"] = symbol
    q["price"] = price
    quotes.append(q)
    p = dict(profile)
    p["symbol"] = symbol
    profiles.append(p)
    charts[symbol] = [dict(day, symbol=symbol) for day in chart]
    ratios_by[symbol] = [dict(r, symbol=symbol) for r in ratios]
    insiders[symbol] = [dict(t, symbol=symbol) for t in insider]

(root / "batch-quote").write_text(json.dumps(quotes))
(root / "quote").write_text(json.dumps([quote]))
(root / "company_profile").write_text(json.dumps(profiles))
(root / "search-symbol").write_text((src / "search_symbol.json").read_text())
(root / "news" / "stock").write_text((src / "stock_news.json").read_text())
for symbol, days in charts.items():
    (root / symbol).mkdir(exist_ok=True)
    (root / symbol / "chart.json").write_text(json.dumps(days))
for symbol, rows in ratios_by.items():
    (root / symbol / "ratios.json").write_text(json.dumps(rows))
for symbol, rows in insiders.items():
    (root / symbol / "insider.json").write_text(json.dumps(rows))
PY

	python3 - "$WORK/root" "$FMP_PORT" <<'PY' &
import http.server, json, sys, urllib.parse
from pathlib import Path

root = Path(sys.argv[1])
ROUTES = {
    "/quote": "quote",
    "/batch-quote": "batch-quote",
    "/profile": "company_profile",
    "/search-symbol": "search-symbol",
    "/news/stock": "news/stock",
}
SYMBOLS = ["AAPL", "MSFT", "NVDA", "TSM", "GEV"]

class Router(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        path = urllib.parse.urlparse(self.path).path
        body = None
        if path in ROUTES:
            body = (root / ROUTES[path]).read_bytes()
        elif path == "/historical-price-eod/full":
            symbol = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).get("symbol", [""])[0].upper()
            if symbol in SYMBOLS:
                body = (root / symbol / "chart.json").read_bytes()
        elif path == "/ratios":
            symbol = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).get("symbol", [""])[0].upper()
            if symbol in SYMBOLS:
                body = (root / symbol / "ratios.json").read_bytes()
        elif path == "/insider-trading/search":
            symbol = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query).get("symbol", [""])[0].upper()
            if symbol in SYMBOLS:
                body = (root / symbol / "insider.json").read_bytes()
        if body is None:
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

http.server.ThreadingHTTPServer(("127.0.0.1", int(sys.argv[2])), Router).serve_forever()
PY
	FIXTURE_PID=$!

	# The store: migrate via one heydayd boot, stop it, seed the watchlist,
	# then the long-lived heydayd for the tests.
	export HEYDAY_ADDR="$ADDR"
	export HEYDAY_DSN="sqlite://$WORK/heydayd.sqlite"
	export FMP_API_KEY="fixture-key"
	export FMP_BASE_URL="http://127.0.0.1:$FMP_PORT/"
	unset HEYDAY_AUTH__SESSION_KEY
	./target/debug/heydayd &
	MIGRATE_PID=$!
	for _ in $(seq 1 100); do
		curl -sf "http://$ADDR/healthz" >/dev/null 2>&1 && break
		sleep 0.1
	done
	kill "$MIGRATE_PID" 2>/dev/null || true
	wait "$MIGRATE_PID" 2>/dev/null || true

	sqlite3 "$WORK/heydayd.sqlite" <<'SQL'
INSERT OR IGNORE INTO watchlists (id, user_id, name, active) VALUES ('0192b58e-0000-7000-8000-000000000001', 'dev', 'Core', 1);
INSERT OR IGNORE INTO watchlist_symbols (watchlist_id, symbol, position) VALUES
  ('0192b58e-0000-7000-8000-000000000001', 'AAPL', 0),
  ('0192b58e-0000-7000-8000-000000000001', 'MSFT', 1),
  ('0192b58e-0000-7000-8000-000000000001', 'NVDA', 2),
  ('0192b58e-0000-7000-8000-000000000001', 'TSM', 3),
  ('0192b58e-0000-7000-8000-000000000001', 'GEV', 4);
INSERT OR IGNORE INTO portfolios (id, user_id, name, kind, thesis) VALUES
  ('0192b58e-0000-7000-8000-000000000101', 'dev', 'AI Infrastructure', 'theme', 'Power and silicon stay the binding constraints on hyperscaler capex.'),
  ('0192b58e-0000-7000-8000-000000000102', 'dev', 'Book', 'holdings', NULL);
INSERT OR IGNORE INTO portfolio_positions (portfolio_id, symbol, position, quantity, avg_cost) VALUES
  ('0192b58e-0000-7000-8000-000000000101', 'NVDA', 0, NULL, NULL),
  ('0192b58e-0000-7000-8000-000000000101', 'TSM', 1, NULL, NULL),
  ('0192b58e-0000-7000-8000-000000000101', 'VRT', 2, NULL, NULL),
  ('0192b58e-0000-7000-8000-000000000101', 'ETN', 3, NULL, NULL),
  ('0192b58e-0000-7000-8000-000000000102', 'NVDA', 0, 100, 120.0),
  ('0192b58e-0000-7000-8000-000000000102', 'TSM', 1, 40, 160.0);
SQL

	export HEYDAY_MARKET_WS="ws://127.0.0.1:17998"
	bun -e '
    const trades = [
      { s: "AAPL", t: 0, type: "T", lp: 233.4, ls: 100 },
      { s: "MSFT", t: 0, type: "T", lp: 416.0, ls: 80 },
      { s: "NVDA", t: 0, type: "T", lp: 133.1, ls: 60 },
      { s: "TSM", t: 0, type: "T", lp: 188.8, ls: 40 },
      { s: "GEV", t: 0, type: "T", lp: 513.5, ls: 20 },
    ];
    let nth = 0;
    const server = Bun.serve({
      port: 17998,
      fetch(req, s) {
        if (s.upgrade(req)) return;
        return new Response("ws only");
      },
      websocket: {
        message(ws, message) {
          const frame = JSON.parse(message);
          if (frame.event === "login" && JSON.stringify(frame).includes("fixture-key")) {
            ws.subscribe("pushes");
          }
        },
      },
    });
    setInterval(() => {
      server.publish("pushes", JSON.stringify(trades[nth % trades.length]));
      nth++;
    }, 700);
  ' &
	WS_PID=$!

	./target/debug/heydayd &
	HEYDAYD_PID=$!
	for _ in $(seq 1 100); do
		curl -sf "http://$ADDR/healthz" >/dev/null 2>&1 && break
		sleep 0.1
	done

	trap server_stop EXIT
}

server_stop() {
	# A trap under `set -e`: every stop step must succeed regardless of what
	# it killed or reaped, or the task's exit code lies about the tests.
	[ -n "${HEYDAYD_PID:-}" ] && kill "$HEYDAYD_PID" 2>/dev/null || true
	[ -n "${FIXTURE_PID:-}" ] && kill "$FIXTURE_PID" 2>/dev/null || true
	[ -n "${WS_PID:-}" ] && kill "$WS_PID" 2>/dev/null || true
	wait 2>/dev/null || true
	return 0
}

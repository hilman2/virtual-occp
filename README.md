# virtual-occp

[![CI](https://github.com/hilman2/virtual-occp/actions/workflows/ci.yml/badge.svg)](https://github.com/hilman2/virtual-occp/actions/workflows/ci.yml)
[![Release](https://github.com/hilman2/virtual-occp/actions/workflows/release.yml/badge.svg)](https://github.com/hilman2/virtual-occp/actions/workflows/release.yml)

Virtual OCPP charging station simulator written in Rust. Spawn multiple virtual
wallboxes, talk to any CSMS backend over OCPP 1.6-J or 2.0.1, drive them from a
per-station Web UI or a central Station Manager — all in one multi-platform
binary with no external runtime dependencies.

## Why

Developing and testing OCPP backends or management software typically requires
physical hardware. `virtual-occp` replaces it with fully interactive in-process
stations: plug in a cable, swipe a virtual RFID chip, start and stop
transactions, observe every WebSocket frame live.

## Features

- **OCPP 1.6-J** and **OCPP 2.0.1** (WebSocket, JSON) — both supported per station
- **Multiple stations in one process**, each on its own HTTP port, each with its
  own CSMS endpoint and credentials
- **Station Manager** — optional central Web UI to create, start, stop, edit and
  delete stations at runtime
- **Per-station Web UI** — plug / unplug, virtual RFID tags, manual start/stop,
  fault injection, live OCPP frame log
- **Authentication** — HTTP Basic Auth for the WebSocket upgrade, `wss://` via
  built-in rustls
- **Persistent state** — all configuration and per-station state as JSON in
  `data/` (no database required)
- **Proactive messaging** — automatic `BootNotification`, periodic `Heartbeat`,
  `StatusNotification` on state change, periodic `MeterValues` while charging
- **CSMS → CP calls** — responds to `Reset`, `ChangeAvailability`,
  `RemoteStartTransaction`, `RemoteStopTransaction`, `TriggerMessage`,
  `UnlockConnector`, `GetConfiguration`, `ChangeConfiguration`,
  `GetVariables`, `SetVariables`, `RequestStartTransaction`,
  `RequestStopTransaction`, `GetBaseReport`
- **Automatic reconnect** with exponential backoff (1s → 30s)
- **Single static binary** — frontend assets are embedded via `rust-embed`

## Quickstart

Download a prebuilt binary from the [releases page](../../releases) or build
from source:

```bash
cargo build --release
```

Run a single station that connects to your CSMS:

```bash
./virtual-occp --station cp1:8080:1.6:ws://localhost:9000/ocpp
```

Open `http://localhost:8080` in your browser.

### Multiple stations + Station Manager

```bash
./virtual-occp \
  --manager-port 8000 \
  --station cp1:8080:1.6:ws://localhost:9000/ocpp \
  --station cp2:8081:2.0.1:ws://localhost:9000/ocpp
```

- `http://localhost:8000` — Station Manager (create/start/stop stations at runtime)
- `http://localhost:8080` / `:8081` — per-station dashboards

### Basic Auth

Either inline in the CSMS URL:

```bash
./virtual-occp --station 'cp1:8080:2.0.1:wss://admin:secret@csms.example.com/ocpp'
```

Credentials are extracted from the URL and sent as
`Authorization: Basic <base64(user:pass)>` during the WebSocket upgrade. You
can also set them as separate fields via the Manager UI or the JSON API.

Passwords are stored in `data/manager.json` (plaintext — this is a developer
tool). The HTTP API never echoes the password back, only a `has_password` flag.

## Station CLI format

```
--station <id>:<http_port>:<version>:<csms_url>
```

- `id` — charge point identity, appended to the CSMS URL as a trailing path
  segment (e.g. `.../ocpp/cp1`)
- `http_port` — port for the per-station Web UI
- `version` — `1.6` or `2.0.1`
- `csms_url` — `ws://…` or `wss://…`, may include `user:password@`

## HTTP APIs

### Per-station (`:<http_port>`)

| Method | Path | Body | Purpose |
|---|---|---|---|
| GET | `/api/state` | — | full state snapshot |
| GET | `/api/events` | — | Server-Sent Events stream |
| POST | `/api/plug` | `{connector_id}` | plug in cable |
| POST | `/api/unplug` | `{connector_id}` | unplug cable |
| POST | `/api/swipe` | `{connector_id, id_tag}` | present RFID chip |
| POST | `/api/stop` | `{connector_id, reason?}` | stop transaction |
| POST | `/api/boot` | — | resend `BootNotification` |
| POST | `/api/reconnect` | — | drop + reopen WebSocket |
| POST | `/api/heartbeat_interval` | `{seconds}` | override heartbeat period |
| POST | `/api/tags` | `{id_tag, label, status}` | add RFID tag |
| DELETE | `/api/tags/:id_tag` | — | remove RFID tag |
| POST | `/api/fault` | `{connector_id, faulted}` | inject / clear fault |
| POST | `/api/meter` | `{connector_id}` | force a `MeterValues` push |

### Station Manager (`:<manager_port>`)

| Method | Path | Purpose |
|---|---|---|
| GET | `/api/manager/stations` | list with running status |
| POST | `/api/manager/stations` | create + optionally start |
| PUT | `/api/manager/stations/:id` | update (stops first if running) |
| POST | `/api/manager/stations/:id/start` | start |
| POST | `/api/manager/stations/:id/stop` | stop |
| DELETE | `/api/manager/stations/:id` | remove (state file on disk is kept) |

## Data layout

```
data/
  manager.json        # registry of stations (Station Manager)
  <station-id>.json   # full state per station (connectors, tags, history, log)
```

Everything is JSON. Delete the directory to reset all simulated stations.

## Development

```bash
# Run fast checks locally
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features

# Run the app
cargo run -- --manager-port 8000
```

CI runs format, clippy, and tests on every push / PR (see
`.github/workflows/ci.yml`).

## Release artifacts

Pushing a `v*` tag triggers `.github/workflows/release.yml`, which builds the
following artifacts and attaches them to the GitHub Release:

| Target | Archive |
|---|---|
| `x86_64-unknown-linux-gnu` | `.tar.gz` |
| `aarch64-unknown-linux-gnu` | `.tar.gz` |
| `x86_64-apple-darwin` (Intel) | `.tar.gz` |
| `aarch64-apple-darwin` (Apple Silicon) | `.tar.gz` |
| `x86_64-pc-windows-msvc` | `.zip` |

Every archive ships with a matching `.sha256` file.

```bash
git tag v0.1.0
git push origin v0.1.0
```

## License

Dual-licensed under MIT OR Apache-2.0.

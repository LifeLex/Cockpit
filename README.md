# Cockpit

Local-first desktop tool (Rust + Tauri 2) that takes a Linear project to merged PRs through a single review loop with two gates: an optional project-level **plan gate** and a per-PR **diff gate**.

See [SPEC.md](SPEC.md) for the full design and [CLAUDE.md](CLAUDE.md) for contributor conventions.

## Prerequisites

- **Rust** stable (edition 2024, minimum 1.85) — installed via [rustup](https://rustup.rs/)
- **Tauri CLI**: `cargo install tauri-cli --version '^2'`
- **Node.js** 22+ and npm
- **Tauri 2 system dependencies** (one-time):
  - **macOS**: Xcode Command Line Tools (`xcode-select --install`)
  - **Ubuntu/Debian**: `sudo apt-get install -y libgtk-3-dev libwebkit2gtk-4.1-dev libappindicator3-dev librsvg2-dev patchelf`
  - **Windows**: WebView2 (ships with Windows 11; install from [Microsoft](https://developer.microsoft.com/en-us/microsoft-edge/webview2/) on Windows 10)
- **GitHub CLI** (`gh`) — authenticated, used by adapters for PR operations
- **Claude Code CLI** (`claude`) — used by the agent adapter to dispatch rework

## Quick start

```bash
# Clone
git clone <repo-url> && cd cockpit

# Install frontend dependencies
cd app && npm install && cd ..

# Run the desktop app (dev mode with hot-reload)
cd app && cargo tauri dev
```

## Repository layout

```
cockpit/
├── crates/
│   └── cockpit-core/       # Headless library: domain model, Gated loop, adapters
├── app/
│   ├── src-tauri/          # Tauri 2 Rust shell
│   └── src/                # React + TypeScript frontend (Vite)
├── SPEC.md                 # What to build
└── CLAUDE.md               # How to build it
```

`cockpit-core` is the source of truth for all logic. The Tauri app is a thin shell that delegates to core, and the headless core integration tests exercise the loop end to end. Core has no UI dependencies.

## Running

### Desktop app

```bash
cd app && cargo tauri dev
```

This starts the Vite dev server on `localhost:5173` and opens the Tauri window. The React frontend hot-reloads; the Rust backend recompiles on changes to `src-tauri/`.

### Frontend only (no Tauri)

```bash
cd app && npm run dev
```

Opens the React app in a browser at `localhost:5173`. IPC calls to the Rust backend will fail, but this is useful for layout and styling work.

## Development

```bash
# Format
cargo fmt --all

# Lint (warnings are errors)
cargo clippy --all-targets --all-features -- -D warnings

# Test (core unit tests + headless e2e integration tests)
cargo test --all

# TypeScript type-check
cd app && npx tsc --noEmit
```

### IPC type generation

Domain types in `cockpit-core` derive `ts-rs::TS` and auto-export TypeScript bindings to `app/src/bindings/`. If you change a domain type, run `cargo test --all` to regenerate the `.ts` files and verify they compile with `npx tsc --noEmit`.

## Architecture

The review loop is a single `Gated` trait with this state machine:

```
Pending → InReview → Dispatched → Reworked → InReview → … → Approved
```

Both `ProjectPlan` (plan gate) and `Review` (diff gate) implement `Gated`. Comments are ephemeral — cleared on each `Reworked` transition. The agent picks up comments via a deterministic prompt, reworks in its worktree, and pushes. A hook server detects the push and flips state to `Reworked`.

Side effects (merge, comment mirroring, batch approval) always require explicit user confirmation — they never fire automatically.

## License

MIT OR Apache-2.0

# One-For-All (OFA)

A Dynamic Island–style status pill for Windows that follows your AI coding agents
(Claude Code, Codex).

## Layout

```
apps/desktop/          Tauri 2 app
  src-tauri/           Rust: window, local API, session store
  src/                 Svelte + TypeScript: island UI
crates/
  ofa-protocol/        event and API types shared by the app and the CLI
  ofa-core/            session state machine (pure logic, unit-tested)
  ofa-cli/             ofa.exe: hook, setup
docs/                  architecture and design notes
```

## Building

Needs Rust (stable, MSVC toolchain), Node 22 and WebView2 (built into Windows 11).

```powershell
cd apps/desktop
npm install
npx tauri dev        # run the island with live reload
npx tauri build      # installer at target/release/bundle/nsis/OFA_<version>_x64-setup.exe
```

From the repo root:

```powershell
cargo test --workspace
cargo build --release -p ofa-cli   # target/release/ofa.exe
```

The island has no tray icon yet; close it with `Stop-Process -Name ofa-desktop`.

CI runs the same checks on every push to `main` and every pull request, and
uploads the installer and `ofa.exe` as a build artifact.

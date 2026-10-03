# One-For-All (OFA)

A small orb on the edge of your Windows screen that follows your Claude Code
sessions. Hover it and it sinks into the edge with a ripple and opens the
details.

## Installing

Run `OFA_<version>_x64-setup.exe`. It installs for your Windows account only
(no admin needed) into `%LOCALAPPDATA%\OFA`, and:

- connects OFA to Claude Code by running `ofa setup` (your Claude Code
  settings are kept and backed up first),
- adds `ofa` to your PATH, for new terminals.

OFA then sits by the clock: click its tray icon for the settings, or right-click
it to hide the orb or quit. Starting OFA again while it runs opens the settings.
"Start with Windows" is in the settings and is off until you turn it on.

Uninstall it from Windows Settings > Apps. That removes OFA's hooks from Claude
Code and its PATH entry too; Claude Code keeps working as before.

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

Both commands first build `ofa.exe` and copy it to `src-tauri/binaries/`, which
the installer bundles next to the app.

From the repo root (on a fresh clone, run `npm run sidecar` in `apps/desktop`
once first, since the app won't compile without the bundled `ofa.exe`):

```powershell
cargo test --workspace
cargo build --release -p ofa-cli   # target/release/ofa.exe
```

For a release build of the island without the installer, close any running
island first, then:

```powershell
cd apps/desktop
npx tauri build --no-bundle   # target/release/ofa-desktop.exe
```

CI runs the same checks on every push to `main` and every pull request, and
uploads the installer and `ofa.exe` as a build artifact.

## Releasing

The full walkthrough is in [docs/releasing.md](docs/releasing.md).

Installed copies check GitHub Releases for `latest.json` shortly after they
start and then every 6 hours, and offer the update in the settings (About >
Install and restart). Updates are signed, and OFA refuses any that don't match
the public key in `tauri.conf.json`.

1. Once: add the repository secret `TAURI_SIGNING_PRIVATE_KEY` with the
   contents of `%USERPROFILE%\.tauri\ofa-updater.key`. Keep that file safe and
   private: without it no update can be published, and anyone with it could
   sign one.
2. Raise `version` in the root `Cargo.toml` (the app, `ofa.exe` and the
   installer all take it from there) and commit.
3. Tag and push: `git tag v0.2.0` then `git push origin v0.2.0`. The Release
   workflow builds, signs and publishes the installer and `latest.json`.

Releases must be downloadable without signing in, so the repository (or
wherever the releases live) has to be public for updates to reach anyone.

## Running and testing

Run these from the repo root in PowerShell, after building.

1. Add OFA's hooks to Claude Code's user settings (once per PC). Your existing
   settings are kept and backed up first:

   ```powershell
   .\target\release\ofa.exe setup
   ```

2. Start the island. Starting it again while it runs opens the settings.

   ```powershell
   Start-Process .\target\release\ofa-desktop.exe
   ```

3. Check it is running:

   ```powershell
   .\target\release\ofa.exe status
   Invoke-RestMethod http://127.0.0.1:47821/health
   ```

4. In a new terminal, in any folder, run `claude` and try:

   | Do this | The orb should |
   | --- | --- |
   | Type `hello` | show Working, then Done for 6 seconds |
   | Ask it to create a file, then hover the orb and click **Allow** (or press Ctrl+Alt+Y) | pulse amber, then create the file without you touching the terminal |
   | Ask it to create a file, then press Ctrl+Alt+N | pass your denial to Claude |
   | Ask it to create a file and answer in the terminal | clear once Claude moves on |
   | Start a long task, then press Esc | go quiet |
   | While it works, click another app, then click the session row | bring the terminal to the front |
   | Close the terminal mid-task | mark the session Lost within about 10 seconds |

If the orb doesn't react, `ofa hook` logs why:

```powershell
Get-Content "$env:LOCALAPPDATA\OFA\hook-errors.log" -Tail 10
```

To change settings, hover the orb and click the gear in the pop-up. Changes
apply straight away. They are kept in `%APPDATA%\OFA\settings.json`, which you
can also edit by hand (changes there apply within a second or two):

```json
{
  "island": { "edge": "right", "size": 20 },
  "exact_usage": true
}
```

- `island.edge`: `right` or `left` (halfway down that edge) or `top` (middle of
  the top edge). `island.size`: the orb's size in pixels, 16 to 64.
- `exact_usage`: the pop-up ends with your Claude plan usage. By default it is
  an estimate from Claude Code's logs on this PC. Set this to `true` to show the
  exact percentages Claude shows; OFA then reads Claude Code's saved sign-in to
  ask Claude, and never changes it.

Stop the island (Claude Code keeps working normally without it):

```powershell
Stop-Process -Name ofa-desktop
```

Remove OFA's hooks from Claude Code completely:

```powershell
.\target\release\ofa.exe setup --uninstall
```

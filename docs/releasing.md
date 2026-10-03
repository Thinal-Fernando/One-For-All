# How OFA releases and updates work

## The short version

You push a tag such as `v0.2.0`. GitHub builds OFA, signs the installer and
publishes it as a release. Copies of OFA that people have installed notice the
new release, and offer it under Settings > About. When they click **Install and
restart**, OFA updates itself and starts again.

## The pieces

| Piece | Where | What it does |
| --- | --- | --- |
| Version | root `Cargo.toml`, `[workspace.package] version` | The one version number. The app, `ofa.exe` and the installer all use it. |
| Updater settings | `apps/desktop/src-tauri/tauri.conf.json`, `plugins.updater` | The **public key** and the address OFA asks: `https://github.com/Thinal-Fernando/One-For-All/releases/latest/download/latest.json` |
| Update checks | `apps/desktop/src-tauri/src/updates.rs` | Asks that address 30 seconds after OFA starts and every 6 hours. Shows the answer in Settings > About and the tray tooltip. Installs only when clicked. |
| Release-only setting | `apps/desktop/src-tauri/tauri.release.conf.json` | Turns on making signature files. Only releases use it, so everyday builds don't need the key. |
| Release workflow | `.github/workflows/release.yml` | Runs on GitHub when you push a tag starting with `v`. |
| Installer steps | `apps/desktop/src-tauri/windows/hooks.nsh` | After installing or updating, runs `ofa setup --path`. On uninstall (not on update) it removes the hooks and the PATH entry. |

## What happens when you push `v0.2.0`

1. GitHub starts the **Release** workflow on a Windows machine.
2. It builds `ofa.exe`, the app and the installer, `OFA_0.2.0_x64-setup.exe`.
3. It signs the installer with your **private key**, taken from the secret
   `TAURI_SIGNING_PRIVATE_KEY`, which gives `OFA_0.2.0_x64-setup.exe.sig`.
4. It creates the release **OFA v0.2.0** on GitHub with three files:
   - `OFA_0.2.0_x64-setup.exe`: what people download the first time
   - `OFA_0.2.0_x64-setup.exe.sig`: the signature
   - `latest.json`: what installed copies read, for example:

     ```json
     {
       "version": "0.2.0",
       "notes": "See the commits since the last release.",
       "pub_date": "2026-10-10T12:00:00Z",
       "platforms": {
         "windows-x86_64": {
           "signature": "dW50cnVzdGVk...",
           "url": "https://github.com/Thinal-Fernando/One-For-All/releases/download/v0.2.0/OFA_0.2.0_x64-setup.exe"
         }
       }
     }
     ```

5. An installed OFA 0.1.0 reads `latest.json`, sees that 0.2.0 is newer, and
   shows "Version 0.2.0 is ready" in Settings > About.
6. On **Install and restart**, it downloads the installer and checks the
   signature against the public key built into it. If the signature doesn't
   match, it refuses. Otherwise it runs the installer with a small progress
   bar. The installer updates the files, reconnects Claude Code, and starts OFA
   again.

## The signing key

Signing proves an update really came from you. There are two halves:

- The **private key** signs. It's in `%USERPROFILE%\.tauri\ofa-updater.key`
  on your PC and must stay secret. Anyone who has it could sign an update
  that every installed OFA would accept.
- The **public key** checks signatures. It's already in `tauri.conf.json` and
  is safe to share, since it can't sign anything.

GitHub needs the private key to sign releases, so you give it to GitHub as an
encrypted **secret**. Do this once:

1. Open the key file and copy everything in it:

   ```powershell
   notepad "$env:USERPROFILE\.tauri\ofa-updater.key"
   ```

   Press Ctrl+A, then Ctrl+C, and close Notepad without saving.

2. On GitHub, open the One-For-All repository and go to **Settings > Secrets
   and variables > Actions**, then click **New repository secret**.
3. For **Name**, enter `TAURI_SIGNING_PRIVATE_KEY`. For **Secret**, paste the
   key. Click **Add secret**.

GitHub never shows a secret again, even to you, and never puts it in logs.
Making the repository public does not expose secrets.

Also keep a backup of the key file somewhere safe, such as a password manager
or a USB stick. If it's lost, a new key can be made, but OFA copies already
installed only trust the old one, so they'd have to be reinstalled by hand
once.

The key has no password. That's fine while it only lives on your PC and in
GitHub's secrets.

## Publishing a release, step by step

Example: going from 0.1.0 to 0.2.0.

1. Make sure everything you want in the release is committed and pushed to
   `main`, and CI is green.
2. In the root `Cargo.toml`, change `version = "0.1.0"` to `version = "0.2.0"`.
   Then, from the repo root, commit and push:

   ```powershell
   cargo check
   git add Cargo.toml Cargo.lock
   git commit -m "Release 0.2.0"
   git push
   ```

   (`cargo check` updates `Cargo.lock` with the new version.)
3. Tag that commit and push the tag:

   ```powershell
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. On GitHub, open the **Actions** tab and watch the **Release** run. When it's
   green, the release appears under **Releases** with its three files.
5. Installed copies pick it up within 6 hours or the next time they start.
   To get it straight away, open Settings > About and click **Check now**.

Rules of thumb:

- The tag must match the version: `v0.2.0` for `0.2.0`.
- Each release must have a higher version than the last. OFA only offers
  updates that are newer than what it runs.
- If a release run fails, fix the problem, then delete and push the tag again:

  ```powershell
  git tag -d v0.2.0
  git push origin :refs/tags/v0.2.0
  ```

  Then tag again (step 3). Also delete the half-made release on GitHub if one
  was created.

## The very first release

Nobody has OFA installed yet, so the first release (for example `v0.1.0`, the
current version) is simply where people download the installer. Updates start
mattering from the second release on.

Updates only reach people once the repository is public, because installed
copies download `latest.json` and the installer without signing in to GitHub.

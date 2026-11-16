# GitHub Actions: automatic builds and the tokens they need

`.github/workflows/build.yml` does the following:

| When | What it does |
|---|---|
| Every push to `main` and every pull request | Rust tests (`cargo test --workspace`) |
| Every push to `main` | After the tests, the Docker image (amd64) is pushed as `ghcr.io/<user>/peekaround:main` |
| A tag like `v2.6.0` | Tests, Windows (`Setup.exe` + portable zip), Linux (`tar.gz`), Android APK, then a **GitHub Release**. A tag with a `-` (`v2.6.0-beta.1`) is published as a pre-release. The Docker image is built for amd64 and arm64 and pushed with the version number and as `latest` (a pre-release leaves `latest` alone) |
| A tag like `build-test` (starting with `build-`) | The same packages without a Release; they are attached to the run's "Artifacts" section |
| Actions tab → Build → **Run workflow** | A test build without a tag; if `release_tag` is filled in, the packages are uploaded to that existing release |

The Docker image is published independently of the Release: even if the Docker job fails, the packages are still attached to the Release. The arm64 image is built on GitHub's arm64 runner, without emulation.

## 1. Which token is needed?

**The workflow needs no personal token.** GitHub gives every run a `GITHUB_TOKEN` automatically. The Release is created with it; the permissions are written in the workflow file.

Check once: repo → **Settings → Actions → General**

- *Actions permissions*: "Allow all actions and reusable workflows" must be selected.
- *Workflow permissions*: the default "Read repository contents" is fine. The jobs that need write access (release, Docker image) ask for it themselves.

## 2. A token for pushing from your computer (if needed)

When pushing over HTTPS, Git asks for a token instead of a password. If "Git Credential Manager" on Windows signs you in through the browser, you do not need to create one.

If you need a token:

1. On GitHub, profile picture at the top right → **Settings → Developer settings → Personal access tokens → Fine-grained tokens → Generate new token**.
2. Fill in the form like this:
   - **Token name:** `PeekAround push`
   - **Expiration:** for example 90 days
   - **Repository access:** *Only select repositories* → `PeakAround`
   - **Permissions → Repository permissions:**
     - **Contents:** Read and write
     - **Workflows:** Read and write. Without it you cannot push files under `.github/workflows/`; GitHub answers "refusing to allow a Personal Access Token to create or update workflow".
     - **Metadata:** Read-only (added automatically)
3. Press **Generate token** and copy the token. It is never shown again.
4. On the first `git push` enter:
   - username: your GitHub username
   - password: the token

With a classic token (**Tokens (classic)**) tick the `repo` and `workflow` boxes.

## 3. Signed Android APK (recommended, free)

Every APK on Android is signed. What matters is that it is always signed **with the same key**: a phone installs a new version over the old one only if both are signed with the same key.

Without the secrets the workflow produces a **debug**-signed APK. On GitHub's machines that debug key is created anew on every run, so every release has a different signature and the phone says "app not installed / package conflicts". You then have to uninstall the old one and install again, which deletes the app's settings and pairing.

To avoid this, create your own key once. It is free; you do not need to buy a certificate:

1. **Create a key.** `keytool` comes with Android Studio or the JDK, for example `C:\Program Files\Android\Android Studio\jbr\bin\keytool.exe` on Windows.

```bash
keytool -genkeypair -v -keystore peekaround-release.keystore -alias peekaround -keyalg RSA -keysize 4096 -validity 10000
```

   Enter the password and details it asks for. **Do not lose this file or the password.** If the key changes, the app on the phone cannot be updated; it has to be uninstalled and installed again.

2. **Turn the file into base64 text.**

   In Windows PowerShell (the text is copied to the clipboard):

```powershell
[Convert]::ToBase64String([IO.File]::ReadAllBytes("peekaround-release.keystore")) | Set-Clipboard
```

   On Linux or macOS:

```bash
base64 -w0 peekaround-release.keystore
```

3. **Add the secrets.** Repo → **Settings → Secrets and variables → Actions → New repository secret**:

| Name | Value |
|---|---|
| `ANDROID_KEYSTORE_BASE64` | the base64 text from step 2 |
| `ANDROID_KEYSTORE_PASSWORD` | the keystore password |
| `ANDROID_KEY_ALIAS` | `peekaround` (the name you gave keytool with `-alias`) |
| `ANDROID_KEY_PASSWORD` | the key password (the same as the keystore password if keytool did not ask separately) |

4. From the next tag on, the Release has `PeekAround-<version>-android-release.apk`. If a debug APK is installed on the phone, uninstall it once and reinstall, because the signature changes.

## 4. Windows signing (optional, usually paid)

Without secrets the Windows `.exe` files are **unsigned**. The app works, but on first launch SmartScreen shows "Windows protected your PC" ("More info" → "Run anyway").

Ways to remove the warning:

| Option | Cost | Note |
|---|---|---|
| **SignPath Foundation** (signpath.org) | Free for open-source projects | Needs an application and approval. A public GPL repo qualifies. Signing is done with SignPath's own GitHub Action; once approved, that step is added to the workflow. |
| **Azure Trusted Signing** (Microsoft) | A few dollars a month | Identity validation for individuals is not available in every country; registering as an individual from Turkey may not be possible. |
| **OV/EV code signing certificate** (Sectigo, DigiCert, Certum…) | About $100–400 a year | Even with a new certificate, SmartScreen may warn for a while until the app has enough downloads. |

The workflow has no signing step yet; with a certificate it would have to be added to the Windows job.

## 5. Publishing a release

```bash
git tag v2.6.0
```

```bash
git push origin v2.6.0
```

**Build** runs in the Actions tab. When it finishes, the packages appear on the **Releases** page.

- The tag's version must match `version` in `Cargo.toml` (`v2.6.0` ↔ `2.6.0`); otherwise the Release job stops.
- To rebuild a tag from scratch:

```bash
git push origin :refs/tags/v2.6.0
```

```bash
git tag -f v2.6.0
```

```bash
git push origin v2.6.0
```

## Troubleshooting
- **Tests fail:** click the red step in Actions and look for the `FAILED` or `panicked` line.
- **No Release was created:** a Release is created only for tags starting with `v`. The tests and the Windows, Linux and Android jobs must all succeed.

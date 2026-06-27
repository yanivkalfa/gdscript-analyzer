# Release & distribution — step-by-step

This guide takes you from zero to a published `v0.1.0`. Everything is automated by GitHub Actions;
you only do a **one-time setup** (three tokens, one npm org, one repo setting). After that, a release
is just "merge a PR."

---

## 0. What gets published, and where

| # | Artifact | Registry / host | Workflow that publishes it |
|---|---|---|---|
| 1 | **7 Rust crates** — `gdscript-base`, `-syntax`, `-api`, `-db`, `-hir`, `-scene`, `-ide` | **crates.io** | `release-plz.yml` |
| 2 | **`@gdscript-analyzer/core`** (the Node addon) + its 6 per-platform sub-packages `@gdscript-analyzer/core-<platform>` | **npm** | `release-napi.yml` |
| 3 | **`@gdscript-analyzer/wasm`** (the browser build) | **npm** | `release-wasm.yml` |
| 4 | **`gdscript` + `gdscript-lsp`** binaries (archives + install scripts) | **GitHub Release** | `release.yml` (cargo-dist) |
| 5 | The mdBook (`/`) + the playground (`/playground/`) | **GitHub Pages** | `pages.yml` |

> **It is NOT one publish per package.** Each *registry* has **one** workflow:
> - crates.io: one `release-plz` run publishes **all 7 crates** (in dependency order).
> - npm core: one `release-napi` run builds every platform and publishes `core` **+ all 6 platform
>   sub-packages**.
> - npm wasm: one `release-wasm` run publishes `wasm`.
> - binaries: one `dist` run publishes **both** binaries.
>
> And **one tag drives all of them.** When you merge the Release PR, `release-plz` pushes the
> `v0.1.0` tag, and that single tag fires #2, #3, and #4 in parallel. (#1 and #5 run on the
> push-to-master itself.)

The crates carrying `publish = false` (`gdscript-cli`, `-ffi`, `-lsp`, `-session`, `-wasm`) are
**not** on crates.io — they ship via npm (the bindings) or GitHub Releases (the binaries).

---

## 1. Accounts you need

| Account | Have it? | Sign up |
|---|---|---|
| **GitHub** (`yanivkalfa`) | ✅ yes | — |
| **crates.io** | log in once | <https://crates.io> → **"Log in with GitHub"** (no separate password; it uses your GitHub account) |
| **npm** | create if you don't have one | <https://www.npmjs.com/signup> |

---

## 2. The three tokens (and an npm org)

You create **3 secret tokens** + **1 npm organization**. Each is below with the exact place to go and
what to set. (Why these three: the GitHub one lets the release **trigger** the others; the crates.io
and npm ones let the workflows **publish**.)

### 2a. npm organization `gdscript-analyzer` — create this FIRST

The npm packages are **scoped** (`@gdscript-analyzer/...`). On npm a scope is either your username or
an **organization**; since `gdscript-analyzer` ≠ your username, it must be an org.

1. Go to **<https://www.npmjs.com/org/create>**.
2. Org name: **`gdscript-analyzer`**.
3. Plan: choose **Free** ("Unlimited public packages", $0).
4. Create. (Now the `@gdscript-analyzer` scope exists and you can publish into it.)

> **Don't want an org?** The only alternative is to rename the packages to your **user scope**
> (`@yanivkalfa/gdscript-core`, `@yanivkalfa/gdscript-wasm`) — no org needed. If you prefer that, tell
> me and I'll change `bindings/node/package.json` + the two npm workflows accordingly. (Everything
> below assumes the `@gdscript-analyzer` org.)

### 2b. `NPM_TOKEN` — lets the workflows publish to npm

1. Go to **<https://www.npmjs.com>** → click your avatar (top-right) → **"Access Tokens"**
   (direct: `https://www.npmjs.com/settings/<your-username>/tokens`).
2. **"Generate New Token"** → choose **"Granular Access Token"** (classic tokens were retired).
3. Fill in:
   - **Token name:** `gdscript-analyzer CI`
   - **Expiration:** the maximum allowed (you'll rotate it; calendar a reminder).
   - **Packages and scopes** → **Permissions: Read and write** → **Select scopes** → choose
     **`@gdscript-analyzer`** (the org you just made). This grants write to every package in the org,
     including ones that don't exist yet (so the *first* publish works).
   - **Organizations:** **No access** (publishing doesn't need org-management rights).
4. **"Generate Token"** → **copy it now** (npm shows it once). You'll paste it in step 3.

### 2c. `CARGO_REGISTRY_TOKEN` — lets the workflow publish to crates.io

1. Go to **<https://crates.io>** → log in with GitHub → click your avatar → **"Account Settings"**
   (direct: `https://crates.io/settings/tokens`).
2. **"New Token"**. Fill in:
   - **Name:** `gdscript-analyzer CI`
   - **Scopes / Endpoints:** check **`publish-new`** *and* **`publish-update`** (needs `publish-new`
     because the crates don't exist on crates.io yet).
   - **Crate scopes** (if offered): restrict to **`gdscript-*`** (least privilege).
   - **Expiration:** set as you like (no-expiry is allowed; rotating is safer).
3. **"Generate Token"** → **copy it now**.

### 2d. `RELEASE_PLZ_TOKEN` — a GitHub token so the tag triggers the other workflows

This is a **GitHub Personal Access Token (PAT)** — a token of *your GitHub account*, used by the
release workflow to push the version tag and open the Release PR. **Why it's required:** GitHub
deliberately blocks the built-in `GITHUB_TOKEN` from triggering other workflows (anti-loop), so a tag
it pushes would *not* fire the napi/wasm/binary publishes. A real PAT does fire them.

1. Go to **<https://github.com/settings/personal-access-tokens/new>** (Settings → Developer settings
   → Personal access tokens → **Fine-grained tokens** → **Generate new token**).
2. Fill in:
   - **Token name:** `release-plz gdscript-analyzer`
   - **Expiration:** up to a year (rotate when it expires).
   - **Resource owner:** `yanivkalfa`.
   - **Repository access:** **"Only select repositories"** → select **`gdscript-analyzer`**.
   - **Permissions → Repository permissions:**
     - **Contents:** **Read and write** (push the tag + commits).
     - **Pull requests:** **Read and write** (open/update the Release PR).
     - (*Metadata: Read-only* is added automatically — leave it.)
3. **"Generate token"** → **copy it now**.

---

## 3. Add the three tokens as repo secrets

1. Go to **<https://github.com/yanivkalfa/gdscript-analyzer/settings/secrets/actions>**
   (repo → **Settings** → **Secrets and variables** → **Actions**).
2. Click **"New repository secret"** and add each of these (Name must match **exactly**):

   | Name | Value |
   |---|---|
   | `RELEASE_PLZ_TOKEN` | the GitHub PAT from **2d** |
   | `CARGO_REGISTRY_TOKEN` | the crates.io token from **2c** |
   | `NPM_TOKEN` | the npm token from **2b** |

---

## 4. Turn on GitHub Pages (Actions source)

1. Go to **<https://github.com/yanivkalfa/gdscript-analyzer/settings/pages>**.
2. Under **"Build and deployment" → "Source"**, select **"GitHub Actions"** (NOT "Deploy from a
   branch").
3. That's it — the site deploys to `https://yanivkalfa.github.io/gdscript-analyzer/` on the next push
   to `master` (docs at `/`, playground at `/playground/`).

---

## 5. Cut the release

Once steps 1–4 are done (and the GA pipeline PR is merged to `master`):

1. **Push/merge anything to `master`.** `release-plz` automatically opens a **Release PR** titled
   like *"chore: release v0.1.0"* — it bumps the version to `0.1.0` and writes `CHANGELOG.md`.
2. **Review + merge that Release PR.**
3. On merge, automatically:
   - `release-plz` publishes the **7 crates** to crates.io and pushes the **`v0.1.0`** tag.
   - the tag fires, in parallel: **`@gdscript-analyzer/core`** (+ platform packages),
     **`@gdscript-analyzer/wasm`**, and the **`gdscript`/`gdscript-lsp`** binaries (GitHub Release).
   - **Pages** redeploys.

> **First-run reality:** cross-compile/publish quirks only surface when the workflows actually run.
> Expect a **red check or two on the first `v0.1.0`** — paste me the failing log and I'll fix it on a
> branch, same as we've been doing. This is normal release shakeout, not a design problem.

### crates.io first-publish gotchas (lessons from `v0.1.0`)

Two crates.io rules bite the **very first** publish of a fresh workspace — neither recurs once the
crates exist:

- **A verified email is required.** crates.io rejects publishing with
  `403 … A verified email address is required` until your account email is set **and verified** at
  <https://crates.io/settings/profile>. Do this *before* cutting the first release.
- **New-crate rate limit: a burst of 5, then 1 every 10 minutes.** Publishing a workspace with **>5
  brand-new crate names** (we have 7) returns `429 Too Many Requests: published too many new crates`
  partway through. This applies only to new crate **names** — publishing new *versions* of existing
  crates is generous, so it **never recurs** after the first release.

**Recovery (what we did for `v0.1.0`):** the `release` job is **idempotent** — it checks crates.io and
skips already-published crates. On a 429, wait until the reset time printed in the error (~10 min) and
**re-run the failed job** (Actions → the run → *Re-run failed jobs*); it resumes from the first
unpublished crate. Repeat until all are up.

**To skip the wait** on a release that adds **many new crate names**, email **help@crates.io**
beforehand for a one-off rate-limit increase (routinely granted for multi-crate workspaces).

---

## 6. Verify it worked

- **crates.io:** <https://crates.io/crates/gdscript-ide> shows `0.1.0`.
- **npm:** <https://www.npmjs.com/package/@gdscript-analyzer/core> and `.../@gdscript-analyzer/wasm`
  show `0.1.0`.
- **Binaries:** the repo's **Releases** page has `v0.1.0` with `.tar.xz`/`.zip` + `*-installer.sh/ps1`.
- **Pages:** <https://yanivkalfa.github.io/gdscript-analyzer/> (docs) and `.../playground/` (live
  analyzer).

---

## 7. Later: tighten security (optional)

Tokens are the simplest way to ship the first release. Once the packages exist, you can switch to
**tokenless OIDC Trusted Publishing** (no long-lived secrets) and delete `CARGO_REGISTRY_TOKEN` /
`NPM_TOKEN`:

- **crates.io:** each crate → Settings → **Trusted Publishing** → add GitHub (owner `yanivkalfa`,
  repo `gdscript-analyzer`, workflow `release-plz.yml`). Then drop `CARGO_REGISTRY_TOKEN` from
  `release-plz.yml` (it already has `id-token: write`).
- **npm:** each package → Settings → **Trusted Publisher** → GitHub Actions (org, repo, workflow
  filename). Then drop `NPM_TOKEN`.

---

## 8. Scope notes for 0.1.0

- The npm Node addon ships **6 platforms** (macOS x64/arm64, Windows x64, Linux x64/arm64-gnu, wasm) —
  the reliable set. musl, armv7, and Windows-arm64 are easy to add later to
  `bindings/node/package.json`'s `napi.targets` **and** the `release-napi.yml` matrix (they must match).
- The browser package is **`--target web`** only. `bundler`/`nodejs` variants can follow if library
  consumers need them.

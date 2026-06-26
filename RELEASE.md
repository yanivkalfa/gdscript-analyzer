# Release & distribution

`gdscript-analyzer` ships through five automated GitHub Actions pipelines, all driven off a single
`v<version>` tag that **release-plz** pushes. One shared version (crates.io + npm + binaries) moves
in lockstep.

| Pipeline | Workflow | Produces | Trigger |
|---|---|---|---|
| crates.io | `release-plz.yml` | `gdscript-ide` + its deps (`-base/-syntax/-api/-db/-hir/-scene`) | push to `master` (Release PR → merge) |
| npm — Node | `release-napi.yml` | `@gdscript-analyzer/core` (per-platform `.node`) | the `v*` tag |
| npm — browser | `release-wasm.yml` | `@gdscript-analyzer/wasm` (wasm-pack `--target web`) | the `v*` tag |
| binaries | `release.yml` (cargo-dist) | `gdscript` + `gdscript-lsp` archives + installers, on the GitHub Release | the `v*` tag |
| Pages | `pages.yml` (`docs.yml`) | the mdBook (`/`) + the playground (`/playground/`) | push to `master` |

**Ownership split (so two tools don't race on the GitHub Release):** release-plz owns version +
changelog + tag + crates.io publish (`git_release_enable = false` in `release-plz.toml`); **dist owns
the GitHub Release** + the binary uploads.

---

## One-time setup (repo owner)

Most failures come from skipping one of these. ⚠️ = easy-to-miss.

### Secrets (repo → Settings → Secrets and variables → Actions)

| Secret | Why | How |
|---|---|---|
| ⚠️ **`RELEASE_PLZ_TOKEN`** | A PAT or GitHub App token. **Required** — the default `GITHUB_TOKEN` does **not** trigger other workflows, so the tag release-plz pushes would never fire napi/wasm/dist. | A fine-grained PAT (or `actions/create-github-app-token`) with **Contents: write** + **Pull requests: write** on this repo. |
| **`NPM_TOKEN`** | The **first** publish of each scoped package (OIDC can't bootstrap a never-published package). After that you can switch to npm Trusted Publishing and drop it. | npmjs.com → Access Tokens → **Granular Access Token** (classic tokens were revoked Dec 2025): Packages+scopes = Read/Write on `@gdscript-analyzer`, Organizations = No access, **Bypass 2FA**. Max 90-day life — rotate. |

### Registries & settings

1. ⚠️ **Create the npm org `gdscript-analyzer`** at <https://www.npmjs.com/org/create> (free for public
   packages). The `@gdscript-analyzer` scope must exist *before* the first publish.
2. ⚠️ **Pages → Source = "GitHub Actions"** (repo → Settings → Pages), **not** "Deploy from a branch".
3. **crates.io Trusted Publishing** (no token): for **each** public crate (`gdscript-ide`, `-base`,
   `-syntax`, `-api`, `-db`, `-hir`, `-scene`) → crates.io → the crate → Settings → Trusted
   Publishing → add GitHub: Owner `yanivkalfa`, Repo `gdscript-analyzer`, Workflow `release-plz.yml`.
   ⚠️ A crate must exist on crates.io first (see bootstrap), so do this *after* the first publish.

### Bootstrap (OIDC can't do a package's very first publish)

Do these **once**, locally, then everything is automated:

```sh
# crates.io: publish each public crate once (dependency order), then add its trusted publisher above.
cargo publish -p gdscript-base   # then -syntax, -api, -db, -hir, -scene, -ide

# npm: publish each scoped package once with a token, then add its npm Trusted Publisher.
cd bindings/node  && npm run build && npm publish --access public   # @gdscript-analyzer/core
wasm-pack build --target web --release --out-dir pkg bindings/wasm
cd bindings/wasm/pkg && npm pkg set name='@gdscript-analyzer/wasm' && npm publish --access public
```

(After bootstrap, configure each package's **npm Trusted Publisher** — npmjs.com → package →
Settings → Trusted Publisher → GitHub Actions, org `gdscript-analyzer`, repo, workflow filename — and
you can stop needing `NPM_TOKEN`.)

---

## Cutting a release (the normal flow)

1. Land your changes on `master` (Conventional Commits drive the version bump).
2. **release-plz** opens/updates a **Release PR** (version bump + changelog). Review + merge it.
3. On merge, release-plz **publishes the crates to crates.io** (OIDC) and **pushes `v<version>`**.
4. That tag fires, in parallel: **release-napi** (`@gdscript-analyzer/core`), **release-wasm**
   (`@gdscript-analyzer/wasm`), and **dist** (`gdscript`/`gdscript-lsp` binaries + the GitHub
   Release). **Pages** redeploys on the same push.

Expect a short **iterate-on-CI** pass on the first release (cross-compile/publish quirks surface only
when the workflows actually run) — that's the normal release shakeout.

### Scope note for 0.1.0
- The napi matrix ships **6 targets** (darwin x64/arm64, win x64, linux x64/arm64-gnu, wasm) — the
  reliable set. musl/zigbuild, armv7, and win-arm64 are easy to add back to
  `bindings/node/package.json` `napi.targets` + `release-napi.yml` once the flow is proven.
- The browser package is **`--target web`** only (the playground/ESM consumer); `bundler`/`nodejs`
  variants can follow.

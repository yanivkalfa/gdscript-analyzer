# Security Policy

We take the security of `gdscript-analyzer` seriously. Thank you for helping
keep the project and its users safe.

## Supported versions

The project is in the `0.x` line. Security fixes are released against the
**latest published minor** only; there are no long-term-support branches while
we are pre-1.0. Please upgrade to the latest release before reporting.

| Version | Supported |
|---|---|
| latest `0.x` minor | :white_check_mark: |
| any older `0.x` release | :x: |

Once the project reaches `1.0`, this policy will be revised to define a
supported-version window.

## Reporting a vulnerability

**Please do not report security vulnerabilities through public GitHub issues,
discussions, or pull requests.**

Report privately using **GitHub Private Vulnerability Reporting**:

1. Go to the repository's **Security** tab:
   <https://github.com/yanivkalfa/gdscript-analyzer/security>
2. Click **Report a vulnerability** (under "Advisories") to open a private
   security advisory.
3. Provide as much detail as you can — affected version(s) and target (Rust
   crate, the napi/Node package, or the wasm package), a description of the
   issue and its impact, and a minimal reproduction (ideally a GDScript snippet
   or input that triggers it).

If you are unable to use Private Vulnerability Reporting, you may email
**yanivkalfa@gmail.com** with the same information. Please do not disclose the
issue publicly until a fix has been released and we have coordinated disclosure.

## Response expectations

- **Acknowledgement:** within **3 business days** of your report.
- **Initial assessment / triage:** within **7 business days**.
- We will keep you informed of progress, work with you on a coordinated
  disclosure timeline, and credit you in the advisory and release notes (unless
  you prefer to remain anonymous).

As a maintainer-led `0.x` community project, please treat these as good-faith
targets rather than contractual guarantees.

## Advisory tooling in CI

The supply chain is monitored on every change and on a schedule:

- **`cargo deny check`** runs in CI — it scans the [RustSec advisory
  database](https://rustsec.org/) for known-vulnerable dependencies and enforces
  the license allow-list and crate bans (configured in `deny.toml`).
- **`cargo-audit`** advisory checks run against the committed `Cargo.lock`.
- **Dependabot** opens automated update PRs for the `cargo`, `npm`, and
  `github-actions` ecosystems.

## Safe harbor

We consider good-faith security research that respects this policy — avoiding
privacy violations, data destruction, and service disruption — to be authorized.
We will not pursue or support legal action against researchers who comply with
it.

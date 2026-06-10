# gh-secrets

A small Rust CLI for managing GitHub Actions repository secrets in bulk.

You keep a local config of profiles, included/excluded repositories, and global
+ per-repository secret values. `gh-secrets sync` then pushes only the secrets
that have changed since the last sync.

## Install

From a clone:

```sh
cargo install --path .
```

## Usage

```sh
gh-secrets --help

# Tell it about your GitHub token.
gh-secrets token <ghp_xxx>

# Add repositories to the profile (or discover all of yours).
gh-secrets repo add owner/repo
gh-secrets repo bootstrap

# Add a secret (global to the profile, or scoped to one repo).
gh-secrets secrets add MY_KEY "value"
gh-secrets secrets add MY_KEY "override-for-this-repo" owner/repo

# Push changed secrets to GitHub.
gh-secrets secrets sync

# See what would change.
gh-secrets check
```

Config lives under `$XDG_CONFIG_HOME/gh-secrets` (Linux), `~/Library/Application
Support/gh-secrets` (macOS), or `%APPDATA%\gh-secrets` (Windows). Override with
`GH_SECRETS_HOME`.

## Develop

```sh
just bootstrap
just check
```

Commits and PR titles must follow [Conventional
Commits](https://www.conventionalcommits.org/) (`feat:`, `fix:`, …) — CI
enforces it and releases are cut automatically from these messages.

See [AGENTS.md](./AGENTS.md) for the invariants and conventions this repo holds
itself to.

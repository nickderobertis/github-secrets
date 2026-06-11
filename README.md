# gh-secrets

A small Rust CLI that syncs secrets from a **source** to one or more
**destinations**, pushing only what changed since the last sync.

Every store declares a read/write capability:

| Store                       | As source (`--from`) | As destination (`--to`) |
| --------------------------- | -------------------- | ----------------------- |
| `github:<owner>/<repo>`     | ✗ (write-only)       | ✓ GitHub Actions secrets |
| `bitwarden`                 | ✓                    | not yet                 |
| `env:<path>` (dotenv file)  | ✓                    | ✓                       |
| `local` (encrypted store)   | ✓                    | ✓                       |

The pipeline (source → secrets → destinations) comes from a checked-in
`gh-secrets.json`, from CLI arguments, or a mix: each `--from`/`--to`/`--secret`
argument replaces that section of the config, so anything a config can express
is also a one-liner.

## Install

Download the latest prebuilt binary, verify its SHA-256 checksum, and drop it
on your PATH (Linux/macOS, and Windows under Git Bash / MSYS / WSL):

```sh
curl -fsSL https://raw.githubusercontent.com/nickderobertis/github-secrets/master/scripts/install.sh | sh
```

Pin a version or pick the install directory:

```sh
curl -fsSL https://raw.githubusercontent.com/nickderobertis/github-secrets/master/scripts/install.sh \
  | sh -s -- --version v0.1.0 --to ~/.local/bin
```

The script defaults to `~/.local/bin`; set `GITHUB_TOKEN` to avoid the GitHub
API rate limit when resolving the latest release. For native Windows PowerShell,
or from a clone:

```sh
cargo install gh-secrets --locked   # from crates.io
cargo install --path .              # from a clone
```

## Usage

```sh
gh-secrets --help

# Scaffold a project config (gh-secrets.json, checked in — it holds mappings,
# never values), then sync it.
gh-secrets init
gh-secrets sync

# Or skip the config entirely — arguments express the same pipeline:
gh-secrets sync --from bitwarden --to github:owner/repo \
  --secret STRIPE_KEY --secret API_KEY=my-bw-item#fields.API_KEY

# Push a local dotenv file's values to several repos:
gh-secrets sync --from env:.env.master \
  --to github:owner/repo1 --to github:owner/repo2 --secret STRIPE_KEY

# Pull your manifest's secrets into a local .env for development:
gh-secrets sync --to env:.env

# Keep ad-hoc values in the global encrypted store and push from it:
gh-secrets store set MY_KEY            # value read from stdin/prompt
gh-secrets sync --from local --to github:owner/repo --secret MY_KEY

# See what a sync would push (read-only, no GitHub token needed):
gh-secrets check

# Discover what the source offers (names/ids only, never values):
gh-secrets source list

# Store credentials (encrypted at rest) as the lowest-priority fallback;
# resolution order is: shell env > .env > .env.local > stored config.
gh-secrets auth github <ghp_xxx>
gh-secrets auth bitwarden --client-id ... --client-secret ... --master-password ...
gh-secrets auth status

# Unlock the vault for a week so nothing prompts for the passphrase
# (a session, like `bw unlock`); end it early with `auth lock`.
gh-secrets auth unlock          # or --days N
gh-secrets auth lock
```

`sync`/`check`/`list` use `./gh-secrets.json` when present and otherwise fall
back to the global config (`gh-secrets init --global`), so `check` is
project-local inside a project and global everywhere else; `--global`/`--config`
force either.

The encrypted vault (stored credentials + the `local` store) and the global
config live under `$XDG_CONFIG_HOME/gh-secrets` (Linux),
`~/Library/Application Support/gh-secrets` (macOS), or `%APPDATA%\gh-secrets`
(Windows). Override with `GH_SECRETS_HOME`. The vault unlocks via an active
session (`auth unlock`, or started automatically the first time you type the
passphrase at a prompt), then `GH_SECRETS_PASSPHRASE`, then an interactive
prompt.

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

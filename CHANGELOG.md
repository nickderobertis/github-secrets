# Changelog

## [1.0.1](https://github.com/nickderobertis/github-secrets/compare/v1.0.0...v1.0.1) (2026-06-11)


### Bug Fixes

* list supported selectors when a Bitwarden field selector is unknown ([#37](https://github.com/nickderobertis/github-secrets/issues/37)) ([b8698aa](https://github.com/nickderobertis/github-secrets/commit/b8698aa1dd27451e7c23ca7a2972c12a1c00386f))

## [1.0.0](https://github.com/nickderobertis/github-secrets/compare/v0.4.0...v1.0.0) (2026-06-11)


### ⚠ BREAKING CHANGES

* the profile-based command surface is gone and `manifest sync|list|init` are now top-level `sync`/`list`/`init`. The gh-secrets.json schema is unchanged. Stored credentials must be re-entered via `gh-secrets auth` (now encrypted).

### Features

* unify the CLI into one sync pipeline of capability-typed stores ([#32](https://github.com/nickderobertis/github-secrets/issues/32)) ([b826dc0](https://github.com/nickderobertis/github-secrets/commit/b826dc0e0179ec767001f93c41742ec3c0fab20e))

## [0.4.0](https://github.com/nickderobertis/github-secrets/compare/v0.3.0...v0.4.0) (2026-06-11)


### Features

* map source secret to differently-named/multiple destination names ([#30](https://github.com/nickderobertis/github-secrets/issues/30)) ([434200f](https://github.com/nickderobertis/github-secrets/commit/434200f9dfaa902cd8770da0b61a5482cc11543e))

## [0.3.0](https://github.com/nickderobertis/github-secrets/compare/v0.2.0...v0.3.0) (2026-06-11)


### Features

* add `source list` to enumerate the manifest's source (Bitwarden vault) ([#28](https://github.com/nickderobertis/github-secrets/issues/28)) ([01852fb](https://github.com/nickderobertis/github-secrets/commit/01852fb9d2a13f4e0bf691289ac14056f9dee9db))

## [0.2.0](https://github.com/nickderobertis/github-secrets/compare/v0.1.2...v0.2.0) (2026-06-11)


### Features

* list available secrets in the CLI (profile and manifest) ([#26](https://github.com/nickderobertis/github-secrets/issues/26)) ([b59f56e](https://github.com/nickderobertis/github-secrets/commit/b59f56ecaefb0dc9edd4f8c0c7cab7ba5fc0bd68))

## [0.1.2](https://github.com/nickderobertis/github-secrets/compare/v0.1.1...v0.1.2) (2026-06-10)


### Bug Fixes

* **install:** abort on Intel macOS instead of 404 on a missing asset ([#20](https://github.com/nickderobertis/github-secrets/issues/20)) ([171b117](https://github.com/nickderobertis/github-secrets/commit/171b1178a0ac856dffa204aaadc7a13b39f189f3))

## [0.1.1](https://github.com/nickderobertis/github-secrets/compare/v0.1.0...v0.1.1) (2026-06-10)


### Chores

* trigger initial automated release ([#17](https://github.com/nickderobertis/github-secrets/issues/17)) ([e5cee39](https://github.com/nickderobertis/github-secrets/commit/e5cee3975c163f84a6f396535bc7c0a65b864393))

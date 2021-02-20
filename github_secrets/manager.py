import datetime
from pathlib import Path
from typing import Optional, Union, Protocol, List
from rich import print

from pyappconf import ConfigFormats

from github_secrets.config import SecretsConfig, Secret
from github_secrets import git
from github_secrets import console_styles as sty
from github_secrets.exc import RepositoryNotInSecretsException, SecretHasNotBeenSyncedException


class HasStr(Protocol):
    def __str__(self) -> str:
        ...


class SecretsManager:
    def __init__(self, config_path: Optional[Union[str, Path]] = None):
        if config_path is None:
            config_path = SecretsConfig._settings.config_location
        else:
            config_path = Path(config_path)
        if config_path.exists():
            self.config = SecretsConfig.load(config_path)
        else:
            config_format = ConfigFormats.from_path(config_path)
            settings = SecretsConfig._settings_with_overrides(
                custom_config_path=config_path.with_suffix(""),
                default_format=config_format,
            )
            self.config = SecretsConfig(settings=settings)

    def add_secret(
        self, name: str, value: HasStr, repository: Optional[str] = None
    ) -> bool:
        secret = Secret(name=name, value=str(value))
        if repository is not None:
            print(
                f"{sty.created()} secret {sty.name_style(name)} for repository {sty.name_style(repository)}"
            )
            created = self.config.repository_secrets.add_secret(secret, repository)
        else:
            print(f"{sty.created()} {sty.global_()} secret {sty.name_style(name)}")
            created = self.config.global_secrets.add_secret(secret)
        return created

    def remove_secret(self, name: str, repository: Optional[str] = None):
        if repository is not None:
            print(
                f"{sty.deleted()} secret {sty.name_style(name)} for repository {sty.name_style(repository)}"
            )
            self.config.repository_secrets.remove_secret(name, repository)
        else:
            print(f"{sty.deleted()} {sty.global_()} secret {sty.name_style(name)}")
            self.config.global_secrets.remove_secret(name)

    def _sync_secret(self, secret: Secret, repo: str):
        try:
            last_synced = self.config.secret_last_synced(secret.name, repo)
        except SecretHasNotBeenSyncedException:
            # Never synced, set to a time before the creation of this package
            last_synced = datetime.datetime(1960, 1, 1)
        if last_synced >= secret.updated:
            print(
                f"Secret {sty.name_style(secret.name)} "
                f"in repository {sty.name_style(repo)} was previously "
                f"synced on {last_synced}, will not update"
            )
            return

        # Do sync
        created = git.update_secret(secret, repo, self.config.github_token)
        self.config.record_sync_for_repo(secret, repo)
        action_str = sty.created() if created else sty.updated()
        print(
            f"{action_str} {sty.global_()} secret {sty.name_style(secret.name)} "
            f"in repository {sty.name_style(repo)}"
        )

    def sync_secret(self, name: str, repository: Optional[str] = None):
        if not self.config.github_token:
            raise ValueError("must set github token before sync")

        repositories: List[str]
        if repository is not None:
            repositories = [repository]
        else:
            repositories = self.config.repositories
        if self.config.global_secrets.has_secret(name):
            print(f"{sty.syncing()} {sty.global_()} secret {sty.name_style(name)}")
            # Global secret, so should update on all repositories
            secret = self.config.global_secrets.get_secret(name)
            for repo in repositories:
                self._sync_secret(secret, repo)
        else:
            print(f"{sty.syncing()} {sty.local()} secret {sty.name_style(name)}")
            # Local secret, need to update only on repositories which include it
            for repo in repositories:
                try:
                    if not self.config.repository_secrets.repository_has_secret(name, repo):
                        continue
                except RepositoryNotInSecretsException:
                    continue
                secret = self.config.repository_secrets.get_secret(name, repo)
                self._sync_secret(secret, repo)

    def bootstrap_repositories(self):
        self.config.bootstrap_repositories()
        self.save()

    def set_token(self, token: str):
        self.config.github_token = token

    def save(self):
        self.config.save()

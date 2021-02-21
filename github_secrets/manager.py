import datetime
from pathlib import Path
from typing import Optional, Union, List
from typing_extensions import Protocol
from rich import print

from rich.markdown import Markdown
from rich.table import Table

from github_secrets.config import SecretsConfig, Secret
from github_secrets import git
from github_secrets import console_styles as sty
from github_secrets.exc import (
    RepositoryNotInSecretsException,
    SecretHasNotBeenSyncedException,
)
from github_secrets import exc


class HasStr(Protocol):
    def __str__(self) -> str:
        ...


class SecretsManager:
    def __init__(self, config_path: Optional[Union[str, Path]] = None):
        self.config: SecretsConfig = SecretsConfig.load_or_create(config_path)

    def add_secret(
        self, name: str, value: HasStr, repository: Optional[str] = None
    ) -> bool:
        secret = Secret(name=name, value=str(value))
        if repository is not None:
            created = self.config.repository_secrets.add_secret(secret, repository)
            created_str = sty.created() if created else sty.updated()
            print(
                f"{created_str} secret {sty.name_style(name)} for repository {sty.name_style(repository)}"
            )
        else:
            created = self.config.global_secrets.add_secret(secret)
            created_str = sty.created() if created else sty.updated()
            print(f"{created_str} {sty.global_()} secret {sty.name_style(name)}")
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

    def _sync_secret(self, secret: Secret, repo: str, verbose: bool = False):
        try:
            last_synced = self.config.secret_last_synced(secret.name, repo)
        except SecretHasNotBeenSyncedException:
            # Never synced, set to a time before the creation of this package
            last_synced = datetime.datetime(1960, 1, 1)
        if last_synced >= secret.updated:
            if verbose:
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

    def sync_secret(self, name: str, repository: Optional[str] = None, verbose: bool = False):
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
                # Check if there is a local repo version, which would take
                # precedence over the global version
                try:
                    use_secret = self.config.repository_secrets.get_secret(
                        secret.name, repo
                    )
                except (
                    exc.RepositoryNotInSecretsException,
                    exc.RepositorySecretDoesNotExistException,
                ):
                    use_secret = secret
                self._sync_secret(use_secret, repo, verbose=verbose)
        else:
            print(f"{sty.syncing()} {sty.local()} secret {sty.name_style(name)}")
            # Local secret, need to update only on repositories which include it
            for repo in repositories:
                try:
                    if not self.config.repository_secrets.repository_has_secret(
                        name, repo
                    ):
                        continue
                except RepositoryNotInSecretsException:
                    continue
                secret = self.config.repository_secrets.get_secret(name, repo)
                self._sync_secret(secret, repo, verbose=verbose)

    def sync_secrets(self, repository: Optional[str] = None, verbose: bool = False):
        print(f"{sty.syncing()} all secrets")
        for sync_config in self.config.sync_configs:
            self.sync_secret(sync_config.secret_name, repository=repository, verbose=verbose)

    def bootstrap_repositories(self):
        new_repos = self.config.bootstrap_repositories()
        for repo in new_repos:
            print(f"{sty.included()} repository {sty.name_style(repo)}")

    def set_token(self, token: str):
        self.config.github_token = token

    def add_repository(self, name: str) -> bool:
        try:
            self.config.add_repository(name)
        except exc.RepositoryAlreadyExistsException:
            print(
                f"Repository {sty.name_style(name)} " f"already exists, will not update"
            )
            return False
        except exc.RepositoryIsExcludedException:
            print(
                f"Repository {sty.name_style(name)} "
                f"is in excluded repositories. "
                f"Remove from excluded before adding to included."
            )
            return False

        print(f"{sty.included()} repository {sty.name_style(name)}")
        return True

    def remove_repository(self, name: str) -> bool:
        try:
            self.config.remove_repository(name)
        except exc.RepositoryDoesNotExistException:
            print(
                f"Repository {sty.name_style(name)} " f"does not exist, cannot remove"
            )
            return False
        print(f"{sty.deleted()} repository {sty.name_style(name)}")
        return True

    def add_exclude_repository(self, name: str) -> bool:
        try:
            self.config.add_exclude_repository(name)
        except exc.RepositoryIsExcludedException:
            print(f"Repository {sty.name_style(name)} " f"is already excluded")
            return False
        except exc.RepositoryIsIncludedException:
            print(
                f"Repository {sty.name_style(name)} "
                f"is in included repositories, cannot add to "
                f"excluded. Remove from included first"
            )
            return False
        print(f"{sty.excluded()} repository {sty.name_style(name)}")
        return True

    def remove_exclude_repository(self, name: str) -> bool:
        try:
            self.config.remove_exclude_repository(name)
        except exc.RepositoryDoesNotExistException:
            print(
                f"Repository {sty.name_style(name)} " f"is not in excluded repositories"
            )
            return False
        print(f"{sty.deleted()} exclude for repository {sty.name_style(name)}")
        return True

    def check(self) -> bool:
        if not self.config.github_token:
            raise ValueError("need to set github token")

        new_repos = self.config.new_repositories
        unsync_secrets = self.config.unsynced_secrets

        if not new_repos and not unsync_secrets:
            print(f"{sty.sync_style('Everything is up to date')}")
            return True

        markdown_str = "# Github Secrets Check\n"
        if new_repos:
            markdown_str += "## New Repositories\n"
            markdown_str += "\n".join(
                [f"- {repo}\n" for repo in new_repos]
            )
            markdown_str += '\n'
        if unsync_secrets:
            markdown_str += "\n## Unsynced Secrets"
            table = Table(show_header=True, header_style="bold")
            table.add_column("Repository")
            table.add_column("Secret")
            for sync_config in unsync_secrets:
                table.add_row(sync_config.repository, sync_config.secret_name)
            print(Markdown(markdown_str))
            print(table)
        else:
            print(Markdown(markdown_str))

        return False

    def record_sync_for_all_repos_and_secrets(self):
        self.config.record_sync_for_all_repos_and_secrets()

    def reset_sync_for_all_repos_and_secrets(self):
        self.config.repository_secrets_last_synced = {}

    def save(self):
        print(
            f"{sty.saved()} settings config at path {self.config.settings.config_location}"
        )
        self.config.save()

import datetime
from pathlib import Path
from typing import List, Dict, Optional, Set

from pyappconf import BaseConfig, AppConfig, ConfigFormats
from pydantic import BaseModel, Field

from github_secrets.exc import (
    RepositoryNotInSecretsException,
    RepositorySecretDoesNotExistException,
    GlobalSecretDoesNotExistException,
    SecretHasNotBeenSyncedException,
    ProfileDoesNotExistException,
)
from github_secrets import exc

APP_NAME = "GithubSecrets"


class Secret(BaseModel):
    name: str
    value: str
    created: datetime.datetime = Field(default_factory=lambda: datetime.datetime.now())
    updated: datetime.datetime = Field(default_factory=lambda: datetime.datetime.now())

    def update(self, value: str):
        self.value = value
        self.updated = datetime.datetime.now()


class SyncConfig(BaseModel):
    secret_name: str
    repository: Optional[str] = None

    def __hash__(self):
        return hash((self.secret_name, self.repository))

    @property
    def global_(self) -> bool:
        return self.repository is None


class RepositorySecrets(BaseModel):
    secrets: Dict[str, List[Secret]] = Field(default_factory=lambda: {})

    @property
    def sync_configs(self) -> Set[SyncConfig]:
        all_configs: Set[SyncConfig] = set()
        for repo, secrets in self.secrets.items():
            all_configs.update(
                [SyncConfig(secret_name=sec.name, repository=repo) for sec in secrets]
            )
        return all_configs

    def add_secret(self, secret: Secret, repository: str) -> bool:
        if repository not in self.secrets:
            self.secrets[repository] = []
        if self.repository_has_secret(secret.name, repository):
            self.update_secret(secret, repository)
            return False
        else:
            self.secrets[repository].append(secret)
            return True

    def repository_has_secret(self, name: str, repository: str):
        if repository not in self.secrets:
            raise RepositoryNotInSecretsException(
                f"repository {repository} does not exist"
            )

        for secret in self.secrets[repository]:
            if secret.name == name:
                return True

        return False

    def get_secret(self, name: str, repository: str):
        if repository not in self.secrets:
            raise RepositoryNotInSecretsException(
                f"repository {repository} does not exist"
            )

        for secret in self.secrets[repository]:
            if secret.name == name:
                return secret

        raise RepositorySecretDoesNotExistException(
            f"repository {repository} does not have secret with name {name}"
        )

    def remove_secret(self, name: str, repository: str):
        if repository not in self.secrets:
            raise RepositoryNotInSecretsException(
                f"repository {repository} does not exist"
            )

        new_secrets: List[Secret] = []
        for secret in self.secrets[repository]:
            if secret.name != name:
                new_secrets.append(secret)
        self.secrets[repository] = new_secrets

    def update_secret(self, secret: Secret, repository: str):
        if repository not in self.secrets:
            raise RepositoryNotInSecretsException(
                f"repository {repository} does not exist"
            )
        updated = False
        for existing_secret in self.secrets[repository]:
            if existing_secret.name == secret.name:
                existing_secret.update(secret.value)
                updated = True
                break
        if not updated:
            raise RepositorySecretDoesNotExistException(
                f"no existing secret for {repository} with name {secret.name}"
            )


class GlobalSecrets(BaseModel):
    secrets: List[Secret] = Field(default_factory=lambda: [])

    @property
    def sync_configs(self) -> Set[SyncConfig]:
        return {SyncConfig(secret_name=sec.name) for sec in self.secrets}

    def add_secret(self, secret: Secret):
        if self.has_secret(secret.name):
            self.update_secret(secret)
            return False
        else:
            self.secrets.append(secret)
            return True

    def has_secret(self, name: str):
        for secret in self.secrets:
            if secret.name == name:
                return True

        return False

    def get_secret(self, name: str) -> Secret:
        for secret in self.secrets:
            if secret.name == name:
                return secret

        raise GlobalSecretDoesNotExistException(
            f"secret with name {name} does not exist in global secrets"
        )

    def remove_secret(self, name: str):
        new_secrets: List[Secret] = []
        for secret in self.secrets:
            if secret.name != name:
                new_secrets.append(secret)
        self.secrets = new_secrets

    def update_secret(self, secret: Secret):
        updated = False
        for existing_secret in self.secrets:
            if existing_secret.name == secret.name:
                existing_secret.update(secret.value)
                updated = True
                break
        if not updated:
            raise GlobalSecretDoesNotExistException(
                f"no existing global secret with name {secret.name}"
            )


class SyncRecord(BaseModel):
    secret_name: str
    last_updated: datetime.datetime = Field(
        default_factory=lambda: datetime.datetime.now()
    )


class SecretsConfig(BaseConfig):
    github_token: str = ""
    include_repositories: Optional[List[str]] = None
    exclude_repositories: Optional[List[str]] = None
    global_secrets: GlobalSecrets = Field(default_factory=lambda: GlobalSecrets())
    repository_secrets: RepositorySecrets = Field(
        default_factory=lambda: RepositorySecrets()
    )
    repository_secrets_last_synced: Dict[str, List[SyncRecord]] = Field(
        default_factory=lambda: {}
    )

    _settings = AppConfig(
        app_name=APP_NAME, default_format=ConfigFormats.YAML, config_name="default"
    )

    @property
    def sync_configs(self) -> Set[SyncConfig]:
        return self.global_secrets.sync_configs.union(
            self.repository_secrets.sync_configs
        )

    @property
    def repositories(self) -> List[str]:
        from github_secrets.git import get_repository_names

        if self.include_repositories is not None:
            return self.include_repositories
        if not self.github_token:
            raise ValueError("need to set github token")
        repositories = get_repository_names(self.github_token)
        if self.exclude_repositories is not None:
            repositories = [
                repo for repo in repositories if repo not in self.exclude_repositories
            ]
        return repositories

    @property
    def new_repositories(self) -> List[str]:
        from github_secrets.git import get_repository_names

        if not self.github_token:
            raise ValueError("need to set github token")
        repositories = get_repository_names(self.github_token)
        if self.exclude_repositories:
            repositories = [
                repo for repo in repositories if repo not in self.exclude_repositories
            ]
        if not self.include_repositories:
            new_repositories = repositories
        else:
            new_repositories = [
                repo for repo in repositories if repo not in self.include_repositories
            ]
        return new_repositories

    @property
    def unsynced_secrets(self) -> List[SyncConfig]:
        global_secrets = self.global_secrets.secrets
        all_sync_configs: List[SyncConfig] = []
        for repo in self.repositories:
            if repo in self.repository_secrets.secrets:
                repo_secrets = self.repository_secrets.secrets[repo]
            else:
                repo_secrets = []
            all_secrets = global_secrets + repo_secrets
            repo_unsync_secrets: List[Secret]
            if repo not in self.repository_secrets_last_synced:
                repo_unsync_secrets = all_secrets
            else:
                repo_unsync_secrets = []
                existing_secrets = [
                    sec.secret_name for sec in self.repository_secrets_last_synced[repo]
                ]
                for secret in all_secrets:
                    if secret.name not in existing_secrets:
                        repo_unsync_secrets.append(secret)
                    else:
                        if secret.updated > self.secret_last_synced(secret.name, repo):
                            repo_unsync_secrets.append(secret)
            sync_configs = [
                SyncConfig(secret_name=sec.name, repository=repo)
                for sec in repo_unsync_secrets
            ]
            all_sync_configs.extend(sync_configs)
        return all_sync_configs

    def bootstrap_repositories(self) -> Set[str]:
        from github_secrets.git import get_repository_names

        if not self.github_token:
            raise ValueError("need to set github token")
        repositories = get_repository_names(self.github_token)
        if self.exclude_repositories:
            repositories = [
                repo for repo in repositories if repo not in self.exclude_repositories
            ]
        if self.include_repositories:
            include_repos = self.include_repositories
        else:
            include_repos = []
        new_repositories = set(repositories).difference(include_repos)
        self.include_repositories = repositories
        return new_repositories

    def secret_last_synced(self, name: str, repository: str) -> datetime.datetime:
        if repository not in self.repository_secrets_last_synced:
            raise SecretHasNotBeenSyncedException(
                f"have not previously synced to repository {repository}"
            )
        for record in self.repository_secrets_last_synced[repository]:
            if record.secret_name == name:
                return record.last_updated
        raise SecretHasNotBeenSyncedException(
            f"secret {name} has not been previously synced to repository {repository}"
        )

    def record_sync_for_repo(self, secret: Secret, repository: str) -> bool:
        sync_record = SyncRecord(secret_name=secret.name)
        if repository not in self.repository_secrets_last_synced:
            self.repository_secrets_last_synced[repository] = []
        updated = False
        # Try update
        for record in self.repository_secrets_last_synced[repository]:
            if record.secret_name == sync_record.secret_name:
                record.last_updated = sync_record.last_updated
                updated = True
        if not updated:
            # Create case
            self.repository_secrets_last_synced[repository].append(sync_record)
        return not updated

    def record_sync_for_all_repos_and_secrets(self):
        repos = self.repositories
        for sync_config in self.sync_configs:
            secret = Secret(name=sync_config.secret_name, value="does not matter")
            if sync_config.global_:
                for repo in repos:
                    self.record_sync_for_repo(secret, repo)
            else:
                self.record_sync_for_repo(secret, sync_config.repository)

    def add_repository(self, name: str):
        if self.include_repositories and name in self.include_repositories:
            raise exc.RepositoryAlreadyExistsException(name)
        if self.exclude_repositories and name in self.exclude_repositories:
            raise exc.RepositoryIsExcludedException(name)
        if self.include_repositories is None:
            self.include_repositories = []
        self.include_repositories.append(name)

    def remove_repository(self, name: str):
        if not self.include_repositories or name not in self.include_repositories:
            raise exc.RepositoryDoesNotExistException(name)
        self.include_repositories.remove(name)

    def add_exclude_repository(self, name: str):
        if self.include_repositories and name in self.include_repositories:
            raise exc.RepositoryIsIncludedException(name)
        if self.exclude_repositories and name in self.exclude_repositories:
            raise exc.RepositoryIsExcludedException(name)
        if self.exclude_repositories is None:
            self.exclude_repositories = []
        self.exclude_repositories.append(name)

    def remove_exclude_repository(self, name: str):
        if not self.exclude_repositories or name not in self.exclude_repositories:
            raise exc.RepositoryDoesNotExistException(name)
        self.exclude_repositories.remove(name)

    class Config:
        env_prefix = "GH_SECRETS_"


class Profile(BaseModel):
    name: str
    config_path: Path


DEFAULT_SECRETS_CONFIG_PATH = SecretsConfig._settings_with_overrides(
    config_name="default"
).config_location
DEFAULT_PROFILE = Profile(name="default", config_path=DEFAULT_SECRETS_CONFIG_PATH)


class SecretsAppConfig(BaseConfig):
    current_profile: Profile = DEFAULT_PROFILE
    profiles: List[Profile] = Field(default_factory=lambda: [DEFAULT_PROFILE])

    _settings = AppConfig(
        app_name=APP_NAME, default_format=ConfigFormats.YAML, config_name="app"
    )

    def profile_exists(self, name: str):
        for profile in self.profiles:
            if profile.name == name:
                return True
        return False

    def get_profile(self, name: str) -> Profile:
        for profile in self.profiles:
            if profile.name == name:
                return profile
        raise ProfileDoesNotExistException(f"no profile with name {name}")

    def add_profile(self, name: str, path: Optional[Path] = None):
        if path is None:
            path = SecretsConfig._settings_with_overrides(
                config_name=name
            ).config_location
        profile = Profile(name=name, config_path=path)
        self.profiles.append(profile)

    def set_profile(self, name: str):
        profile = self.get_profile(name)
        self.current_profile = profile

    def delete_profile(self, name: str):
        new_profiles: List[Profile] = []
        for profile in self.profiles:
            if profile.name != name:
                new_profiles.append(profile)
        self.profiles = new_profiles

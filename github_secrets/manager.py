from pathlib import Path
from typing import Optional, Union, Protocol

from pyappconf import AppConfig, ConfigFormats

from github_secrets.config import SecretsConfig, APP_NAME, Secret
from github_secrets.git import get_repository_names


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
                custom_config_path=config_path.with_suffix(''),
                default_format=config_format
            )
            self.config = SecretsConfig(settings=settings)

    def add_secret(self, name: str, value: HasStr, repository: Optional[str] = None):
        secret = Secret(name=name, value=str(value))
        if repository is not None:
            self.config.repository_secrets.add_secret(secret, repository)
        else:
            self.config.global_secrets.add_secret(secret)

    def remove_secret(self, name: str, repository: Optional[str] = None):
        if repository is not None:
            self.config.repository_secrets.remove_secret(name, repository)
        else:
            self.config.global_secrets.remove_secret(name)

    def bootstrap_repositories(self):
        self.config.bootstrap_repositories()
        self.save()

    def set_token(self, token: str):
        self.config.github_token = token

    def save(self):
        self.config.save()

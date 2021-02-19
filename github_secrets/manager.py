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
        config_class = SecretsConfig
        if config_path is not None:

            class CustomSecretsConfig(SecretsConfig):
                _settings = AppConfig(
                    app_name=APP_NAME,
                    custom_config_path=config_path,
                    default_format=ConfigFormats.YAML,
                )

            config_class = CustomSecretsConfig
        if config_class._settings.config_location.exists():
            self.config = config_class.load()
        else:
            self.config = config_class()

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

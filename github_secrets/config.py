from typing import List, Dict

from pyappconf import BaseConfig, AppConfig
from pydantic import BaseModel, Field

APP_NAME = "GithubSecrets"


class Secret(BaseModel):
    name: str
    value: str


class RepositorySecrets(BaseModel):
    secrets: Dict[str, List[Secret]] = Field(default_factory=lambda: {})

    def add_secret(self, secret: Secret, repository: str):
        if repository not in self.secrets:
            self.secrets[repository] = []
        if self.repository_has_secret(secret.name, repository):
            self.remove_secret(secret.name, repository)
        self.secrets[repository].append(secret)

    def repository_has_secret(self, name: str, repository: str):
        if repository not in self.secrets:
            raise ValueError(f"repository {repository} does not exist")

        for secret in self.secrets[repository]:
            if secret.name == name:
                return True

        return False

    def remove_secret(self, name: str, repository: str):
        if repository not in self.secrets:
            raise ValueError(f"repository {repository} does not exist")

        new_secrets: List[Secret] = []
        for secret in self.secrets[repository]:
            if secret.name != name:
                new_secrets.append(secret)
        self.secrets[repository] = new_secrets


class GlobalSecrets(BaseModel):
    secrets: List[Secret] = Field(default_factory=lambda: [])

    def add_secret(self, secret: Secret):
        if self.has_secret(secret.name):
            self.remove_secret(secret.name)
        self.secrets.append(secret)

    def has_secret(self, name: str):
        for secret in self.secrets:
            if secret.name == name:
                return True

        return False

    def remove_secret(self, name: str):
        new_secrets: List[Secret] = []
        for secret in self.secrets:
            if secret.name != name:
                new_secrets.append(secret)
        self.secrets = new_secrets


class SecretsConfig(BaseConfig):
    global_secrets: GlobalSecrets = Field(default_factory=lambda: [])
    repository_secrets: RepositorySecrets = Field(default_factory=lambda: {})

    _settings = AppConfig(app_name=APP_NAME)

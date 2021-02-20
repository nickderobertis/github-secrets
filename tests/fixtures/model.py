import pytest

from github_secrets.app import GithubSecretsApp
from github_secrets.manager import SecretsManager
from tests.config import CONFIG_FILE_PATH_YAML, APP_CONFIG_FILE_PATH_YAML


def get_secrets_manager(**kwargs) -> SecretsManager:
    manager = SecretsManager(config_path=CONFIG_FILE_PATH_YAML, **kwargs)
    return manager


def get_secrets_app(**kwargs) -> GithubSecretsApp:
    app = GithubSecretsApp(config_path=APP_CONFIG_FILE_PATH_YAML, **kwargs)
    return app


@pytest.fixture(scope="function")
def secrets_manager():
    return get_secrets_manager()


@pytest.fixture(scope="function")
def secrets_app():
    return get_secrets_app()

import pytest

from github_secrets.manager import SecretsManager
from tests.config import CONFIG_FILE_PATH_YAML


def get_secrets_manager(**kwargs) -> SecretsManager:
    manager = SecretsManager(config_path=CONFIG_FILE_PATH_YAML, **kwargs)
    return manager


@pytest.fixture(scope='function')
def secrets_manager():
    return get_secrets_manager()
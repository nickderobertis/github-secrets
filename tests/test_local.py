from pathlib import Path

from github_secrets.config import Secret
from github_secrets.manager import SecretsManager
from tests.config import GENERATED_CONFIG_FILE_PATH, CONFIG_FILE_PATH
from tests.fixtures.model import secrets_manager, get_secrets_manager


def test_add_global_secret(secrets_manager: SecretsManager):
    secrets_manager.add_secret("woo", "baby")
    assert secrets_manager.config.global_secrets.secrets[-1] == Secret(
        name="woo", value="baby"
    )


def test_add_repository_secret(secrets_manager: SecretsManager):
    secrets_manager.add_secret("woo", "baby", "my/repo")
    assert secrets_manager.config.repository_secrets.secrets["my/repo"] == [
        Secret(name="woo", value="baby")
    ]


def test_remove_global_secret(secrets_manager: SecretsManager):
    secrets_manager.remove_secret("a")
    assert secrets_manager.config.global_secrets.secrets == []


def test_remove_repository_secret(secrets_manager: SecretsManager):
    secrets_manager.remove_secret("c", "this/that")
    assert secrets_manager.config.repository_secrets.secrets["this/that"] == [
        Secret(name="e", value="f")
    ]


def test_save(secrets_manager: SecretsManager):
    secrets_manager.config.settings.custom_config_path = GENERATED_CONFIG_FILE_PATH
    secrets_manager.save()
    assert (
        Path(str(GENERATED_CONFIG_FILE_PATH) + ".toml").read_text()
        == Path(str(CONFIG_FILE_PATH) + ".toml").read_text()
    )

from pathlib import Path

from github_secrets.config import Secret
from github_secrets.manager import SecretsManager
from tests.config import GENERATED_CONFIG_FILE_PATH, CONFIG_FILE_PATH, GENERATED_CONFIG_FILE_PATH_YAML, \
    CONFIG_FILE_PATH_YAML, TEST_TIME
from tests.conftest import FROZEN
from tests.fixtures.model import secrets_manager, get_secrets_manager


def test_add_global_secret(secrets_manager: SecretsManager):
    assert secrets_manager.add_secret("woo", "baby")
    assert secrets_manager.config.global_secrets.secrets[-1] == Secret(
        name="woo", value="baby"
    )


def test_add_repository_secret(secrets_manager: SecretsManager):
    assert secrets_manager.add_secret("woo", "baby", "my/repo")
    assert secrets_manager.config.repository_secrets.secrets["my/repo"] == [
        Secret(name="woo", value="baby")
    ]


def test_update_global_secret(secrets_manager: SecretsManager):
    expect_secret = Secret(name="a", value="b")
    FROZEN.tick()
    expect_secret.update('c')
    assert not secrets_manager.add_secret('a', 'c')
    assert secrets_manager.config.global_secrets.secrets[-1] == expect_secret
    FROZEN.move_to(TEST_TIME)


def test_update_repository_secret(secrets_manager: SecretsManager):
    expect_secret = Secret(name="c", value="d")
    FROZEN.tick()
    expect_secret.update('e')
    assert not secrets_manager.add_secret('c', 'e', 'this/that')
    assert secrets_manager.config.repository_secrets.secrets['this/that'][0] == expect_secret
    FROZEN.move_to(TEST_TIME)


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
    secrets_manager.set_token('')
    secrets_manager.save()
    assert (
        GENERATED_CONFIG_FILE_PATH_YAML.read_text()
        == CONFIG_FILE_PATH_YAML.read_text()
    )

from github_secrets.manager import SecretsManager
from tests.config import GENERATED_CONFIG_FILE_PATH
from tests.fixtures.model import secrets_manager, get_secrets_manager


def test_bootstrap_repositories(secrets_manager: SecretsManager):
    secrets_manager.config.settings.custom_config_path = GENERATED_CONFIG_FILE_PATH
    secrets_manager.bootstrap_repositories()
    assert len(secrets_manager.config.include_repositories) > 0
    assert (
        "nickderobertis/github-secrets"
        not in secrets_manager.config.include_repositories
    )
    for repo in secrets_manager.config.include_repositories:
        assert "/" in repo

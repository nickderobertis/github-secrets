from unittest.mock import patch, PropertyMock

from github_secrets.config import SyncConfig, SyncRecord
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


def test_new_repositories(secrets_manager: SecretsManager):
    assert secrets_manager.config.new_repositories == [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]


def test_unsynced_secrets(secrets_manager: SecretsManager):
    assert secrets_manager.config.unsynced_secrets == [
        SyncConfig(repository="testghuser/test-repo-1", secret_name="a"),
        SyncConfig(repository="testghuser/test-repo-2", secret_name="a"),
    ]


def test_check(secrets_manager: SecretsManager):
    assert not secrets_manager.check()
    with patch.object(
        secrets_manager.config.__class__,
        "unsynced_secrets",
        new_callable=PropertyMock,
        return_value=[],
    ):
        with patch.object(
            secrets_manager.config.__class__,
            "new_repositories",
            new_callable=PropertyMock,
            return_value=[],
        ):
            assert secrets_manager.check()


def test_record_sync_for_all_repos_and_secrets(secrets_manager: SecretsManager):
    secrets_manager.add_secret("b", "c", "testghuser/test-repo-2")
    secrets_manager.record_sync_for_all_repos_and_secrets()
    sync_record_a = SyncRecord(secret_name="a")
    sync_record_b = SyncRecord(secret_name="b")
    for sync_recs in secrets_manager.config.repository_secrets_last_synced.values():
        sync_recs.sort(key=lambda rec: rec.secret_name)

    assert secrets_manager.config.repository_secrets_last_synced == {
        "testghuser/test-repo-1": [sync_record_a],
        "testghuser/test-repo-2": [sync_record_a, sync_record_b],
        "this/that": [SyncRecord(secret_name="c"), SyncRecord(secret_name="e")],
        "this/who": [SyncRecord(secret_name="g")],
    }


def test_reset_sync_for_all_repos_and_secrets(secrets_manager: SecretsManager):
    secrets_manager.record_sync_for_all_repos_and_secrets()
    secrets_manager.reset_sync_for_all_repos_and_secrets()
    assert secrets_manager.config.repository_secrets_last_synced == {}

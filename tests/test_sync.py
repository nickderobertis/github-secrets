from unittest.mock import patch, Mock

from github_secrets.config import SyncRecord, Secret
from github_secrets.manager import SecretsManager
from tests.config import TEST_TIME
from tests.conftest import FROZEN
from tests.fixtures.model import secrets_manager


@patch("github_secrets.manager.git.update_secret")
def test_sync_global_secret(mock: Mock, secrets_manager: SecretsManager):
    secrets_manager.config.include_repositories = [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]
    secret = secrets_manager.config.global_secrets.get_secret("a")
    FROZEN.tick()
    secrets_manager.sync_secret("a")
    mock.assert_any_call(
        secret, "testghuser/test-repo-1", secrets_manager.config.github_token
    )
    mock.assert_any_call(
        secret, "testghuser/test-repo-2", secrets_manager.config.github_token
    )
    assert mock.call_count == 2
    expect_sync_record = SyncRecord(secret_name="a")
    assert secrets_manager.config.repository_secrets_last_synced[
        "testghuser/test-repo-1"
    ] == [expect_sync_record]
    assert secrets_manager.config.repository_secrets_last_synced[
        "testghuser/test-repo-2"
    ] == [expect_sync_record]
    FROZEN.move_to(TEST_TIME)


@patch("github_secrets.manager.git.update_secret")
def test_global_secret_already_synced(mock: Mock, secrets_manager: SecretsManager):
    secrets_manager.config.include_repositories = [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]
    FROZEN.tick()
    sync_record = SyncRecord(secret_name="a")
    secrets_manager.config.repository_secrets_last_synced["testghuser/test-repo-1"] = [
        sync_record
    ]
    secrets_manager.config.repository_secrets_last_synced["testghuser/test-repo-2"] = [
        sync_record
    ]
    FROZEN.tick()
    secrets_manager.sync_secret("a")
    mock.assert_not_called()
    FROZEN.move_to(TEST_TIME)


@patch("github_secrets.manager.git.update_secret")
def test_sync_local_secret(mock: Mock, secrets_manager: SecretsManager):
    secrets_manager.config.include_repositories = [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]
    secret = Secret(name="temp", value="val")
    secrets_manager.add_secret(secret.name, secret.value, "testghuser/test-repo-1")
    FROZEN.tick()
    secrets_manager.sync_secret("temp")
    mock.assert_any_call(
        secret, "testghuser/test-repo-1", secrets_manager.config.github_token
    )
    assert mock.call_count == 1
    expect_sync_record = SyncRecord(secret_name="temp")
    assert secrets_manager.config.repository_secrets_last_synced[
        "testghuser/test-repo-1"
    ] == [expect_sync_record]
    assert (
        "testghuser/test-repo-2"
        not in secrets_manager.config.repository_secrets_last_synced
    )
    FROZEN.move_to(TEST_TIME)


@patch("github_secrets.manager.git.update_secret")
def test_local_secret_already_synced(mock: Mock, secrets_manager: SecretsManager):
    secrets_manager.config.include_repositories = [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]
    secret = Secret(name="temp", value="val")
    secrets_manager.add_secret(secret.name, secret.value, "testghuser/test-repo-1")
    FROZEN.tick()
    sync_record = SyncRecord(secret_name="temp")
    secrets_manager.config.repository_secrets_last_synced["testghuser/test-repo-1"] = [
        sync_record
    ]
    FROZEN.tick()
    secrets_manager.sync_secret("temp")
    mock.assert_not_called()
    FROZEN.move_to(TEST_TIME)


@patch("github_secrets.manager.git.update_secret")
def test_sync_global_secret_overridden_by_local(mock: Mock, secrets_manager: SecretsManager):
    secrets_manager.config.include_repositories = [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]
    secret = secrets_manager.config.global_secrets.get_secret("a")
    secrets_manager.add_secret(secret.name, 'local version', "testghuser/test-repo-1")
    repo_secret = secrets_manager.config.repository_secrets.get_secret('a', "testghuser/test-repo-1")
    FROZEN.tick()
    secrets_manager.sync_secret("a")
    mock.assert_any_call(
        repo_secret, "testghuser/test-repo-1", secrets_manager.config.github_token
    )
    mock.assert_any_call(
        secret, "testghuser/test-repo-2", secrets_manager.config.github_token
    )
    assert mock.call_count == 2
    expect_sync_record = SyncRecord(secret_name="a")
    assert secrets_manager.config.repository_secrets_last_synced[
        "testghuser/test-repo-1"
    ] == [expect_sync_record]
    assert secrets_manager.config.repository_secrets_last_synced[
        "testghuser/test-repo-2"
    ] == [expect_sync_record]
    FROZEN.move_to(TEST_TIME)


@patch("github_secrets.manager.git.update_secret")
def test_sync_all_secrets(mock: Mock, secrets_manager: SecretsManager):
    secrets_manager.config.include_repositories = [
        "testghuser/test-repo-1",
        "testghuser/test-repo-2",
    ]
    secrets_manager.config.global_secrets.secrets = []
    local_secret = Secret(name="temp", value="val")
    secrets_manager.add_secret(local_secret.name, local_secret.value, "testghuser/test-repo-1")
    global_secret = Secret(name='temp2', value='val2')
    secrets_manager.add_secret(global_secret.name, global_secret.value)
    FROZEN.tick()
    secrets_manager.sync_secrets()
    mock.assert_any_call(
        local_secret, "testghuser/test-repo-1", secrets_manager.config.github_token
    )
    mock.assert_any_call(
        global_secret, "testghuser/test-repo-1", secrets_manager.config.github_token
    )
    mock.assert_any_call(
        global_secret, "testghuser/test-repo-2", secrets_manager.config.github_token
    )
    assert mock.call_count == 3
    expect_local_sync_record = SyncRecord(secret_name="temp")
    expect_global_sync_record = SyncRecord(secret_name="temp2")
    assert sorted(secrets_manager.config.repository_secrets_last_synced[
        "testghuser/test-repo-1"
    ], key=lambda rec: rec.secret_name) == [expect_local_sync_record, expect_global_sync_record]
    assert secrets_manager.config.repository_secrets_last_synced[
               "testghuser/test-repo-2"
           ] == [expect_global_sync_record]
    FROZEN.move_to(TEST_TIME)
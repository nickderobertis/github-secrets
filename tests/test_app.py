from unittest.mock import patch

from github_secrets.app import GithubSecretsApp
from github_secrets.config import Profile
from tests.config import (
    GENERATED_APP_CONFIG_FILE_PATH_YAML,
    GENERATED_CONFIG_FILE_PATH_YAML,
)
from tests.fixtures.model import secrets_app


def test_create_profile(secrets_app: GithubSecretsApp):
    with patch.object(secrets_app.manager, "save") as save_mock:
        with patch.object(secrets_app, 'save'):
            expect_profile = Profile(
                name="woo", config_path=GENERATED_CONFIG_FILE_PATH_YAML
            )
            assert secrets_app.create_profile(
                expect_profile.name, expect_profile.config_path
            )
            assert secrets_app.config.profiles[-1] == expect_profile


def test_create_existing_profile(secrets_app: GithubSecretsApp):
    assert not secrets_app.create_profile("test")


def test_set_profile(secrets_app: GithubSecretsApp):
    with patch.object(secrets_app.manager, "save") as save_mock:
        with patch.object(secrets_app, 'save'):
            expect_profile = Profile(
                name="woo", config_path=GENERATED_CONFIG_FILE_PATH_YAML
            )
            secrets_app.create_profile(expect_profile.name, expect_profile.config_path)
            assert secrets_app.set_profile(expect_profile.name)
            assert secrets_app.config.current_profile == expect_profile


def test_set_non_existent_profile(secrets_app: GithubSecretsApp):
    assert not secrets_app.set_profile("adfg")


def test_delete_profile(secrets_app: GithubSecretsApp):
    with patch.object(secrets_app.manager, "save") as save_mock:
        with patch.object(secrets_app, 'save'):
            expect_profile = Profile(
                name="woo", config_path=GENERATED_CONFIG_FILE_PATH_YAML
            )
            secrets_app.create_profile(expect_profile.name, expect_profile.config_path)
            assert secrets_app.delete_profile(expect_profile.name)
            assert expect_profile not in secrets_app.config.profiles


def test_delete_existent_profile(secrets_app: GithubSecretsApp):
    assert not secrets_app.delete_profile("adfg")


def test_delete_current_profile(secrets_app: GithubSecretsApp):
    assert not secrets_app.delete_profile("test")


def test_save_load(secrets_app: GithubSecretsApp):
    secrets_app.config.settings.custom_config_path = (
        GENERATED_APP_CONFIG_FILE_PATH_YAML.with_suffix("")
    )
    secrets_app.manager.config.settings.custom_config_path = (
        GENERATED_CONFIG_FILE_PATH_YAML.with_suffix("")
    )
    secrets_app.save()
    new_secrets_app = GithubSecretsApp(GENERATED_APP_CONFIG_FILE_PATH_YAML)
    assert new_secrets_app.config == secrets_app.config


def assert_wrapper_method(secrets_app: GithubSecretsApp, attr: str, *args, **kwargs):
    with patch.object(secrets_app.manager, "save") as save_mock:
        with patch.object(secrets_app.manager, attr) as mock:
            method = getattr(secrets_app, attr)
            method(*args, **kwargs)
            mock.assert_called_once_with(*args, **kwargs)
            save_mock.assert_called_once()


def test_wrapper_methods(secrets_app: GithubSecretsApp):
    assert_wrapper_method(secrets_app, "set_token", "abc")
    assert_wrapper_method(
        secrets_app, "add_secret", "woo", "yeah", repository="this/that"
    )
    assert_wrapper_method(secrets_app, "remove_secret", "woo", repository="this/that")
    assert_wrapper_method(secrets_app, "sync_secret", "woo", repository="this/that", verbose=False)
    assert_wrapper_method(secrets_app, "sync_secrets", repository="this/that", verbose=False)
    assert_wrapper_method(secrets_app, "bootstrap_repositories")
    assert_wrapper_method(secrets_app, "add_repository", "woo")
    assert_wrapper_method(secrets_app, "remove_repository", "woo")
    assert_wrapper_method(secrets_app, "add_exclude_repository", "woo")
    assert_wrapper_method(secrets_app, "remove_exclude_repository", "woo")
    assert_wrapper_method(secrets_app, "record_sync_for_all_repos_and_secrets")
    assert_wrapper_method(secrets_app, "reset_sync_for_all_repos_and_secrets")

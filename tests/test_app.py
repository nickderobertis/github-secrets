from github_secrets.app import GithubSecretsApp
from github_secrets.config import Profile
from tests.config import (
    GENERATED_APP_CONFIG_FILE_PATH_YAML,
    GENERATED_CONFIG_FILE_PATH_YAML,
)
from tests.fixtures.model import secrets_app


def test_create_profile(secrets_app: GithubSecretsApp):
    expect_profile = Profile(name="woo", config_path=GENERATED_CONFIG_FILE_PATH_YAML)
    assert secrets_app.create_profile(expect_profile.name, expect_profile.config_path)
    assert secrets_app.config.profiles[-1] == expect_profile


def test_create_existing_profile(secrets_app: GithubSecretsApp):
    assert not secrets_app.create_profile("test")


def test_set_profile(secrets_app: GithubSecretsApp):
    expect_profile = Profile(name="woo", config_path=GENERATED_CONFIG_FILE_PATH_YAML)
    secrets_app.create_profile(expect_profile.name, expect_profile.config_path)
    assert secrets_app.set_profile(expect_profile.name)
    assert secrets_app.config.current_profile == expect_profile


def test_set_non_existent_profile(secrets_app: GithubSecretsApp):
    assert not secrets_app.set_profile("adfg")


def test_delete_profile(secrets_app: GithubSecretsApp):
    expect_profile = Profile(name="woo", config_path=GENERATED_CONFIG_FILE_PATH_YAML)
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
    secrets_app.manager.config.settings.custom_config_path = GENERATED_CONFIG_FILE_PATH_YAML.with_suffix('')
    secrets_app.save()
    new_secrets_app = GithubSecretsApp(GENERATED_APP_CONFIG_FILE_PATH_YAML)
    assert new_secrets_app.config == secrets_app.config


def test_set_token(secrets_app: GithubSecretsApp):
    secrets_app.set_token('abc')
    assert secrets_app.manager.config.github_token == 'abc'

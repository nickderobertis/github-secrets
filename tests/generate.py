"""
Generates initial config file
"""
import os

from freezegun import freeze_time

from tests.config import CONFIG_FILE_PATH, TEST_TIME, APP_CONFIG_FILE_PATH_YAML
from tests.fixtures.model import get_secrets_manager, get_secrets_app

if __name__ == '__main__':
    conf_path = str(CONFIG_FILE_PATH) + '.yaml'
    app_conf_path = APP_CONFIG_FILE_PATH_YAML
    if os.path.exists(conf_path):
        os.remove(conf_path)
    if os.path.exists(app_conf_path):
        os.remove(app_conf_path)

    with freeze_time(TEST_TIME):
        manager = get_secrets_manager()
        manager.config.exclude_repositories = ['nickderobertis/github-secrets']
        manager.add_secret('a', 'b')
        manager.add_secret('c', 'd', repository='this/that')
        manager.add_secret('e', 'f', repository='this/that')
        manager.add_secret('g', 'h', repository='this/who')
        manager.set_token('')  # don't save private github token
        manager.save()

        app = get_secrets_app()
        app.create_profile('test', conf_path)
        app.set_profile('test')
        app.delete_profile('default')
        app.set_token('')
        app.save()

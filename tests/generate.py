"""
Generates initial config file
"""
import os

from tests.config import CONFIG_FILE_PATH
from tests.fixtures.model import get_secrets_manager

if __name__ == '__main__':
    conf_path = str(CONFIG_FILE_PATH) + '.yaml'
    if os.path.exists(conf_path):
        os.remove(conf_path)
    manager = get_secrets_manager()
    manager.config.exclude_repositories = ['nickderobertis/github-secrets']
    manager.add_secret('a', 'b')
    manager.add_secret('c', 'd', repository='this/that')
    manager.add_secret('e', 'f', repository='this/that')
    manager.add_secret('g', 'h', repository='this/who')
    manager.set_token('')  # don't save private github token
    manager.save()

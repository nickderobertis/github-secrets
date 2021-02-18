"""
Generates initial config file
"""
from tests.fixtures.model import get_secrets_manager

if __name__ == '__main__':
    manager = get_secrets_manager()
    manager.add_secret('a', 'b')
    manager.add_secret('c', 'd', repository='this/that')
    manager.add_secret('e', 'f', repository='this/that')
    manager.add_secret('g', 'h', repository='this/who')
    manager.save()

import os

from github_secrets.git import get_repository_names

GH_TOKEN = os.environ['GITHUB_SECRETS_GITHUB_TOKEN']

def test_get_repository_names():
    names = get_repository_names(GH_TOKEN)
    assert len(names) > 0
    for name in names:
        assert '/' in name

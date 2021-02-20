import os

from github_secrets import git

GH_TOKEN = os.environ["GITHUB_SECRETS_GITHUB_TOKEN"]


def test_get_repository_names():
    names = git.get_repository_names(GH_TOKEN)
    assert names == ["testghuser/test-repo-1", "testghuser/test-repo-2"]


def test_get_repository():
    name = "testghuser/test-repo-1"
    repo = git.get_repository(name, GH_TOKEN)
    assert repo.full_name == name
    assert repo.description == 'First test repo'

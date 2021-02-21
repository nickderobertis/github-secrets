from typing import List

from github import Github
from github.Repository import Repository

from github_secrets.config import Secret


def get_repository_names(access_token: str) -> List[str]:
    g = Github(access_token)
    return [repo.full_name for repo in g.get_user().get_repos()]


def get_repository(name: str, access_token: str) -> Repository:
    g = Github(access_token)
    return g.get_repo(name)


def update_secret(secret: Secret, repo_name: str, access_token: str) -> bool:
    """
    :return: Whether secret was newly created (False when just updated)
    """
    repo = get_repository(repo_name, access_token)
    return repo.create_secret(secret.name, secret.value)

# TODO [#1]: once pygithub supports get, delete secret, add functionality and complete tests
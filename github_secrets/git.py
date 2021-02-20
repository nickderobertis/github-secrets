from typing import List

from github import Github
from github.Repository import Repository


def get_repository_names(access_token: str) -> List[str]:
    g = Github(access_token)
    return [repo.full_name for repo in g.get_user().get_repos()]


def get_repository(name: str, access_token: str) -> Repository:
    g = Github(access_token)
    return g.get_repo(name)
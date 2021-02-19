from typing import List

from github import Github


def get_repository_names(access_token: str) -> List[str]:
    g = Github(access_token)
    return [repo.full_name for repo in g.get_user().get_repos()]

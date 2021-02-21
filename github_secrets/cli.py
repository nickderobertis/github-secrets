from pathlib import Path
from typing import Optional, Union

import typer

from github_secrets.app import GithubSecretsApp

cli = typer.Typer()
app: GithubSecretsApp


VERBOSE_DOC = 'Show additional output useful for debugging'


@cli.callback(invoke_without_command=True)
def start_app(ctx: typer.Context):
    """
    Github Secrets CLI
    """
    global app
    app = GithubSecretsApp()
    if not app.config.settings.config_location.exists():
        app.save()


profile_cli = typer.Typer(
    help="Manage the profile for working with secrets. Multiple profiles "
    "are supported to manage secrets and authentication separately"
)


@profile_cli.command(name="create")
def create_profile(
    name: str = typer.Argument(..., help="Profile name"),
    path: Optional[str] = typer.Argument(
        None, help="Path to config file (YAML, JSON, TOML)"
    ),
):
    """
    Create a profile to store and manage secrets
    """
    app.create_profile(name, path=path)


@profile_cli.command(name="delete")
def delete_profile(
    name: str = typer.Argument(..., help="Profile name"),
):
    """
    Delete a profile used for storing and managing secrets.

    NOTE: Cannot be undone! Use carefully.
    """
    app.delete_profile(name)


@profile_cli.command(name="set")
def set_profile(
    name: str = typer.Argument(..., help="Profile name"),
):
    """
    Set the active profile used for storing and managing secrets.
    """
    app.set_profile(name)


cli.add_typer(profile_cli, name="profile")


@cli.command()
def token(token: str = typer.Argument(..., help="Github personal access token")):
    """
    Set the Github token used for authentication
    """
    app.set_token(token)


repo_cli = typer.Typer(
    name="repo", help="Manage included and excluded repositories for current profile"
)

REPO_NAME_DOC = (
    "Full name of repository including owner, e.g. nickderobertis/github-secrets"
)


@repo_cli.command(name="add")
def add_repository(name: str = typer.Argument(..., help=REPO_NAME_DOC)):
    """
    Add a repository to included repositories
    """
    app.add_repository(name)


@repo_cli.command(name="remove")
def remove_repository(name: str = typer.Argument(..., help=REPO_NAME_DOC)):
    """
    Remove a repository from included repositories
    """
    app.remove_repository(name)


@repo_cli.command(name="add-exclude")
def add_exclude_repository(name: str = typer.Argument(..., help=REPO_NAME_DOC)):
    """
    Add a repository to excluded repositories
    """
    app.add_exclude_repository(name)


@repo_cli.command(name="remove-exclude")
def remove_excluded_repository(name: str = typer.Argument(..., help=REPO_NAME_DOC)):
    """
    Remove a repository from excluded repositories
    """
    app.remove_exclude_repository(name)


@repo_cli.command(name="bootstrap")
def boostrap_repos():
    """
    Scans Github to get all repositories for user and add them
    to included repositories if they are not already excluded
    """
    app.bootstrap_repositories()


cli.add_typer(repo_cli)

secrets_cli = typer.Typer(name="secrets", help="Manage secrets for current profile")

SECRET_NAME_DOC = "Name of secret (key)"
SECRET_VALUE_DOC = (
    "Value for secret. Be sure to contain it in quotes if there are spaces."
)


@secrets_cli.command(name="add")
def add_secret(
    name: str = typer.Argument(..., help=SECRET_NAME_DOC),
    value: str = typer.Argument(..., help=SECRET_VALUE_DOC),
    repository: Optional[str] = typer.Argument(None, help=REPO_NAME_DOC),
):
    """
    Add a secret globally (within profile) or for a certain repository
    """
    app.add_secret(name, value, repository=repository)


@secrets_cli.command(name="remove")
def remove_secret(
    name: str = typer.Argument(..., help=SECRET_NAME_DOC),
    repository: Optional[str] = typer.Argument(None, help=REPO_NAME_DOC),
):
    """
    Remove a secret globally (within profile) or for a certain repository
    """
    app.remove_secret(name, repository=repository)


@secrets_cli.command(name="sync")
def sync_secret(
    name: Optional[str] = typer.Argument(None, help=SECRET_NAME_DOC),
    repository: Optional[str] = typer.Argument(None, help=REPO_NAME_DOC),
    verbose: bool = typer.Option(False, '--verbose', '-v', help=VERBOSE_DOC, show_default=False)
):
    """
    Sync one or all secrets to one or all repositories in profile.

    If name is not passed, will sync all secrets. If repository is not
    passed, will sync to all repositories. Pass no arguments
    for a full sync.
    """
    if name is not None:
        app.sync_secret(name, repository=repository, verbose=verbose)
    else:
        app.sync_secrets(repository=repository, verbose=verbose)


cli.add_typer(secrets_cli)

record_cli = typer.Typer(
    name="record", help="Manage records of previous syncs for profile"
)


@record_cli.command(name="fill")
def record_sync_for_all_repos_and_secrets():
    """
    Mark all secrets as having been synced.
    Useful for initial setup when secrets already exist
    """
    app.record_sync_for_all_repos_and_secrets()


@record_cli.command(name="reset")
def reset_sync_for_all_repos_and_secrets():
    """
    Remove all records of syncs. Will cause all secrets to
    be updated on next sync.

    NOTE: this cannot be undone. Use carefully.
    """
    app.reset_sync_for_all_repos_and_secrets()


cli.add_typer(record_cli)


@cli.command(name="check")
def check():
    """
    Shows the repositories not reflected in the config and the unsynced secrets
    """
    app.check()


if __name__ == "__main__":
    cli()

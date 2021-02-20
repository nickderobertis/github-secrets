from pathlib import Path
from typing import Optional, Union

from rich import print

from github_secrets.config import SecretsAppConfig
from github_secrets.manager import SecretsManager
from github_secrets import console_styles as sty


class GithubSecretsApp:

    def __init__(self, config_path: Optional[Union[str, Path]] = None):
        self.config: SecretsAppConfig = SecretsAppConfig.load_or_create(config_path)
        self.manager = self._get_manager()

    def _get_manager(self) -> SecretsManager:
        return SecretsManager(self.config.current_profile.config_path)

    def save(self):
        print(
            f"{sty.saved()} application config at path {self.config.settings.config_location}"
        )
        self.config.save()
        self.manager.save()

    def create_profile(self, name: str, path: Optional[Union[str, Path]] = None) -> bool:
        if name == 'app':
            raise ValueError('app is not allowed as a profile name')

        if path is not None:
            path = Path(path)
        if self.config.profile_exists(name):
            print(
                f"Profile {sty.name_style(name)} "
                f"already exists, will not update"
            )
            return False
        self.config.add_profile(name, path)
        print(
            f"{sty.created()} profile {sty.name_style(name)} with path {path}"
        )
        return True

    def set_profile(self, name: str) -> bool:
        if not self.config.profile_exists(name):
            print(
                f"Profile {sty.name_style(name)} "
                f"does not exist, cannot set"
            )
            return False
        self.config.set_profile(name)
        self.manager = self._get_manager()
        print(
            f"{sty.set_()} profile {sty.name_style(name)} with path {self.config.current_profile.config_path}"
        )
        return True

    def delete_profile(self, name: str) -> bool:
        if not self.config.profile_exists(name):
            print(
                f"Profile {sty.name_style(name)} "
                f"does not exist, cannot delete"
            )
            return False
        profile = self.config.get_profile(name)
        if self.config.current_profile == profile:
            print(
                f"Profile {sty.name_style(name)} "
                f"is set as current profile, cannot delete"
            )
            return False
        self.config.delete_profile(name)
        print(
            f"{sty.deleted()} profile {sty.name_style(name)}"
        )
        return True

    def set_token(self, value: str):
        self.manager.set_token(value)
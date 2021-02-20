class GithubSecretsException(Exception):
    pass


class RepositoryNotInSecretsException(GithubSecretsException):
    pass


class SecretDoesNotExistException(GithubSecretsException):
    pass


class RepositorySecretDoesNotExistException(SecretDoesNotExistException):
    pass


class GlobalSecretDoesNotExistException(SecretDoesNotExistException):
    pass


class SecretHasNotBeenSyncedException(GithubSecretsException):
    pass


class ProfileException(GithubSecretsException):
    pass


class ProfileDoesNotExistException(ProfileException):
    pass


class RepositoryException(GithubSecretsException):
    pass


class RepositoryAlreadyExistsException(GithubSecretsException):
    pass


class RepositoryDoesNotExistException(GithubSecretsException):
    pass


class RepositoryIsExcludedException(GithubSecretsException):
    pass


class RepositoryIsIncludedException(GithubSecretsException):
    pass
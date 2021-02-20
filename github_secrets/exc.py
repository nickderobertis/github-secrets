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

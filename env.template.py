import os

if 'GITHUB_SECRETS_GITHUB_TOKEN' not in os.environ:
    os.environ['GITHUB_SECRETS_GITHUB_TOKEN'] = ''
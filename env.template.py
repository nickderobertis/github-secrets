import os

if 'GH_SECRETS_GITHUB_TOKEN' not in os.environ:
    os.environ['GH_SECRETS_GITHUB_TOKEN'] = ''
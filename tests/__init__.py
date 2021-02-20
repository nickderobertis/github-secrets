# Setup to use freezegun with pyyaml
from freezegun.api import FakeDatetime
from yaml.representer import SafeRepresenter
SafeRepresenter.add_representer(FakeDatetime, SafeRepresenter.represent_datetime)
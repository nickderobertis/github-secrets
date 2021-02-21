from _pytest.main import Session
import freezegun
from freezegun.api import FrozenDateTimeFactory
import os
import shutil

if not os.path.exists('env.py'):
    shutil.copy('env.template.py', 'env.py')
import env  # loads local environment variables
from tests.config import TEST_TIME

FREEZER = freezegun.freeze_time(TEST_TIME)
FROZEN: FrozenDateTimeFactory

def pytest_sessionstart(session: Session):
    global FROZEN
    FROZEN = FREEZER.start()


def pytest_sessionfinish(session: Session):
    FREEZER.stop()
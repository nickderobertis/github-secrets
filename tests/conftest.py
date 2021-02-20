from _pytest.main import Session
import freezegun

import env  # loads local environment variables
from tests.config import TEST_TIME

FREEZER = freezegun.freeze_time(TEST_TIME)

def pytest_sessionstart(session: Session):
    FREEZER.start()


def pytest_sessionfinish(session: Session):
    FREEZER.stop()
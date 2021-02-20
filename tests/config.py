from pathlib import Path

TEST_DIR = Path(__file__).parent
INPUT_DATA_DIR = TEST_DIR / 'input_data'
GENERATED_DATA_DIR = TEST_DIR / 'generated_data'
CONFIG_FILE_PATH = INPUT_DATA_DIR / 'config'
CONFIG_FILE_PATH_YAML = INPUT_DATA_DIR / 'config.yaml'
GENERATED_CONFIG_FILE_PATH = GENERATED_DATA_DIR / 'config'
GENERATED_CONFIG_FILE_PATH_YAML = GENERATED_DATA_DIR / 'config.yaml'

TEST_TIME = "2021-02-20 12:00:00"

if not INPUT_DATA_DIR.exists():
    INPUT_DATA_DIR.mkdir()
if not GENERATED_DATA_DIR.exists():
    GENERATED_DATA_DIR.mkdir()
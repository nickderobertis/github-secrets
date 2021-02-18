from pathlib import Path

TEST_DIR = Path(__file__).parent
INPUT_DATA_DIR = TEST_DIR / 'input_data'
GENERATED_DATA_DIR = TEST_DIR / 'generated_data'
CONFIG_FILE_PATH = INPUT_DATA_DIR / 'config'
GENERATED_CONFIG_FILE_PATH = GENERATED_DATA_DIR / 'config'

if not INPUT_DATA_DIR.exists():
    INPUT_DATA_DIR.mkdir()
if not GENERATED_DATA_DIR.exists():
    GENERATED_DATA_DIR.mkdir()
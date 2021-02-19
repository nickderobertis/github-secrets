#### TEMP
import os
import sys

dropbox_paths = [
    '/home/nick/Dropbox',
    r'D:\Dropbox',
    r'E:\Dropbox (Personal)'
]

# Place at end to be imported first
project_sub_path_parts = [
    ['Python', 'py-app-conf'],
]

for project_sub_paths in project_sub_path_parts:
    project_sub_path = os.path.sep.join(project_sub_paths)
    [sys.path.insert(0, path) for path in [os.path.join(dbp, project_sub_path) for dbp in dropbox_paths]]

### END TEMP
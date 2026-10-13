import { execFileSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
for (const file of readdirSync('.github').filter(f => f.endsWith('.mjs'))) execFileSync(process.execPath, ['--check', `.github/${file}`], { stdio: 'inherit' });

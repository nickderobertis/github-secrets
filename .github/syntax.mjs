// Exit 0: all repository-private JS parses; exit 1: syntax or filesystem error.
import { execFileSync } from 'node:child_process';
import { readdirSync } from 'node:fs';
for (const file of readdirSync('.github').filter(f => f.endsWith('.mjs'))) execFileSync(process.execPath, ['--check', `.github/${file}`], { stdio: 'inherit' });

import { appendFileSync, readFileSync } from 'node:fs';
const policy = JSON.parse(readFileSync(process.argv[2], 'utf8'));
const diagnostic = process.env.GITHUB_EVENT_NAME === 'workflow_dispatch' && process.env.DIAGNOSTIC === 'true';
appendFileSync(process.env.GITHUB_OUTPUT, `macosRequired=${policy.macosRequired || diagnostic}\n`);

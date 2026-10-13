// Exit 0: wrote the routing output; exit 1: invalid input or output failure.
import { policyInput } from './inputs.mjs';
import { appendFileSync, readFileSync } from 'node:fs';
if (process.argv.length !== 3) throw new Error('usage: macos-route.mjs POLICY_JSON; supply the classifier output');
const policy = policyInput(JSON.parse(readFileSync(process.argv[2], 'utf8')));
export const diagnosticRequired = (eventName, diagnostic) => eventName === 'workflow_dispatch' && diagnostic === 'true';
const diagnostic = diagnosticRequired(process.env.GITHUB_EVENT_NAME, process.env.DIAGNOSTIC);

appendFileSync(process.env.GITHUB_OUTPUT, `macosRequired=${policy.macosRequired || diagnostic}\n`);

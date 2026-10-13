// Exit 0: wrote the routing output; exit 1: invalid input or output failure.
import { policyInput } from './inputs.mjs';
import { appendFileSync, existsSync, readFileSync } from 'node:fs';
if (process.argv.length !== 3) throw new Error('usage: macos-route.mjs POLICY_JSON; supply the classifier output');
const policy = policyInput(JSON.parse(readFileSync(process.argv[2], 'utf8')));
const output = process.env.GITHUB_OUTPUT;
if (!output || !existsSync(output)) throw new Error('GITHUB_OUTPUT must name the existing Actions output file; run this inside the policy step');
// The manual diagnostic is its own workflow (macos-diagnostic.yml); CI routes
// on the classifier alone, so no automatic caller can opt in.
appendFileSync(output, `macosRequired=${policy.macosRequired}\n`);

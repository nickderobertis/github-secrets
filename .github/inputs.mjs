// Strict argv parsing for repository-private commands; no permissive fallbacks.
export function options(argv, required) {
  const result = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i], value = argv[i + 1];
    if (!required.includes(key) || key in result || !value || value.startsWith('--')) throw new Error(`invalid argument ${key}; expected ${required.join(' ')}`);
    result[key] = value;
  }
  if (required.some(key => !(key in result))) throw new Error(`missing argument; expected ${required.join(' ')}`);
  return result;
}
export function policyInput(policy) {
  if (!policy || typeof policy !== 'object' || typeof policy.macosRequired !== 'boolean' || typeof policy.isReleaseOnly !== 'boolean' || typeof policy.reason !== 'string') throw new Error('invalid classifier result');
  return policy;
}

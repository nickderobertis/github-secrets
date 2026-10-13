// Exit 0: required proof passed or omission authorized; exit 1: any other outcome.
// GitHub job outcomes are the only doubled boundary in carrier journeys.
const { POLICY_RESULT, MACOS_RESULT, CHECK_RESULT, MACOS_REQUIRED } = process.env;
if (POLICY_RESULT !== 'success' || CHECK_RESULT !== 'success' || !['true', 'false'].includes(MACOS_REQUIRED)) {
  console.error('Mac carrier: prerequisite failed, cancelled or missing; inspect policy and check jobs.');
  process.exit(1);
}
if (MACOS_REQUIRED === 'true' && MACOS_RESULT === 'success') {
  console.log('Mac check, build and install proof passed.');
} else if (MACOS_REQUIRED === 'false' && MACOS_RESULT === 'skipped') {
  console.log('Mac lane omitted by classifier; no Mac assertions ran.');
} else {
  console.error(`Mac carrier: required=${MACOS_REQUIRED}, result=${MACOS_RESULT || 'missing'}; inspect Mac job.`);
  process.exit(1);
}

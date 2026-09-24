// The four plural-form suffixes a key in src/locales/*.json can carry
// (src/i18n.ts's t(key, vars, count) / crates/core/src/i18n.rs's render()
// try key.<form> before key.other before the bare key). Shared by
// scripts/i18n.test.mjs and scripts/check-demo-fixture.test.mjs so the two
// cannot list a different set without one of them failing to compile.
export const PLURAL_SUFFIXES = ["one", "few", "many", "other"];

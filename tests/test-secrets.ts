/**
 * Test secrets, single-homed so the miniflare binding stub (vitest.config.ts) and the
 * seed-encryption key (tests/database.ts) cannot drift apart. Import-free on purpose:
 * vitest.config.ts consumes this module and must not pull in test helpers.
 */

/** Base64 of 32 zero bytes; the AES-256 key every test binding and seeding path uses. */
export const TEST_CREDENTIALS_AES_KEY = "AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=";

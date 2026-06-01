// Derive the wallet's `(r, salt)` blinding pair for split-prove Solution A
// from a BIP39 recovery phrase, mirroring how `derive_midnight_zswap_seed.mjs`
// derives the zswap `sk`. Production-shaped: HKDF-SHA256 over the BIP39 seed
// with a stable domain separator and the wallet's `(account, key_index)`
// identifier. Same recovery phrase → same `(r, salt)` → same `reg_leaf`
// across the `register-wallet` run and every subsequent split spend.
//
// Output: a JSON object with two 64-byte hex strings. The Rust side feeds
// each chunk to `Fr::from_uniform_bytes` so the resulting scalars are
// uniformly distributed over the Jubjub scalar field (no top-bit bias).

import { hkdfSync } from 'node:crypto';
import { mnemonicToSeedSync } from '@scure/bip39';
import { validateMnemonic } from '@midnight-ntwrk/wallet-sdk-hd';

function envValue(name, fallback = '') {
  const value = process.env[name];
  if (value && value.trim()) return value.trim();
  if (name.startsWith('MIDNIGHT_LOCAL_')) {
    const legacyName = `MIDNIGHT_PREVIEW_${name.slice('MIDNIGHT_LOCAL_'.length)}`;
    const legacyValue = process.env[legacyName];
    if (legacyValue && legacyValue.trim()) return legacyValue.trim();
  }
  return fallback;
}

const phrase = envValue('MIDNIGHT_LOCAL_RECOVERY_PHRASE').replace(/\s+/g, ' ');
const accountRaw = envValue('MIDNIGHT_LOCAL_ACCOUNT');
const indexRaw = envValue('MIDNIGHT_LOCAL_ZSWAP_KEY_INDEX');
const account = Number.parseInt(accountRaw || '0', 10);
const index = Number.parseInt(indexRaw || '0', 10);

if (!phrase) {
  throw new Error('MIDNIGHT_LOCAL_RECOVERY_PHRASE is not set');
}
if (!validateMnemonic(phrase)) {
  throw new Error('MIDNIGHT_LOCAL_RECOVERY_PHRASE is not a valid English BIP39 mnemonic');
}
if (!Number.isSafeInteger(account) || account < 0) {
  throw new Error('MIDNIGHT_LOCAL_ACCOUNT must be a non-negative integer');
}
if (!Number.isSafeInteger(index) || index < 0) {
  throw new Error('MIDNIGHT_LOCAL_ZSWAP_KEY_INDEX must be a non-negative integer');
}

const seed = mnemonicToSeedSync(phrase);

const salt = Buffer.from('midnight:split-prove:wallet-blinding-salt[v1]', 'utf8');
const info = Buffer.from(
  `midnight:split-prove:wallet-blinding[v1]:account=${account}:index=${index}`,
  'utf8',
);

// 128 bytes = 64 for `r_uniform` || 64 for `salt_uniform`.
const expanded = Buffer.from(hkdfSync('sha256', seed, salt, info, 128));
const rUniform = expanded.subarray(0, 64);
const saltUniform = expanded.subarray(64, 128);

console.log(
  JSON.stringify({
    rUniformHex: rUniform.toString('hex'),
    saltUniformHex: saltUniform.toString('hex'),
  }),
);

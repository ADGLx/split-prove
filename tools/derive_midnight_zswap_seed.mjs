import { HDWallet, Roles, validateMnemonic } from '@midnight-ntwrk/wallet-sdk-hd';
import { mnemonicToSeedSync } from '@scure/bip39';

const phrase = (process.env.MIDNIGHT_PREVIEW_RECOVERY_PHRASE ?? '').trim().replace(/\s+/g, ' ');
const accountRaw = (process.env.MIDNIGHT_PREVIEW_ACCOUNT ?? '').trim();
const indexRaw = (process.env.MIDNIGHT_PREVIEW_ZSWAP_KEY_INDEX ?? '').trim();
const account = Number.parseInt(accountRaw || '0', 10);
const index = Number.parseInt(indexRaw || '0', 10);

if (!phrase) {
  throw new Error('MIDNIGHT_PREVIEW_RECOVERY_PHRASE is not set');
}
if (!validateMnemonic(phrase)) {
  throw new Error('MIDNIGHT_PREVIEW_RECOVERY_PHRASE is not a valid English BIP39 mnemonic');
}
if (!Number.isSafeInteger(account) || account < 0) {
  throw new Error('MIDNIGHT_PREVIEW_ACCOUNT must be a non-negative integer');
}
if (!Number.isSafeInteger(index) || index < 0) {
  throw new Error('MIDNIGHT_PREVIEW_ZSWAP_KEY_INDEX must be a non-negative integer');
}

const seed = mnemonicToSeedSync(phrase);
const wallet = HDWallet.fromSeed(seed);
if (wallet.type !== 'seedOk') {
  throw new Error(`HD wallet seed derivation failed: ${wallet.error}`);
}

const zswap = wallet.hdWallet.selectAccount(account).selectRole(Roles.Zswap).deriveKeyAt(index);
wallet.hdWallet.clear();

if (zswap.type !== 'keyDerived') {
  throw new Error('Zswap key derivation failed');
}

console.log(Buffer.from(zswap.key).toString('hex'));

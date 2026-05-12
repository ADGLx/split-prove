import * as fs from 'node:fs/promises';
import * as path from 'node:path';
import * as ledger from '@midnight-ntwrk/ledger-v8';
import { MidnightBech32m, ShieldedAddress, UnshieldedAddress } from '@midnight-ntwrk/wallet-sdk-address-format';
import pino, { type Logger } from 'pino';
import {
  type WalletContext,
  mnemonicToSeed,
  initWalletWithSeed,
  waitForSync,
  waitForFunds,
  displayWalletBalances,
  registerNightForDust,
  closeWallet,
} from './wallet.js';
import { type Config } from './config.js';

const NIGHT_AMOUNT = 50_000n * 10n ** 6n; // 50,000 NIGHT in smallest unit
const MAX_ACCOUNTS = 10;

let logger: Logger = pino({ level: 'silent' });

export function setLogger(_logger: Logger): void {
  logger = _logger;
}

export interface FundedAccount {
  name: string;
  unshieldedAddr: string;
  shieldedAddr: string;
  dustAddr: string;
  nightBalance: bigint;
  dustBalance: bigint;
}

interface AccountConfig {
  name: string;
  mnemonic: string;
}

interface AccountsFile {
  accounts: AccountConfig[];
}

/**
 * Transfer NIGHT tokens from master wallet to an unshielded receiver address.
 */
async function transferNight(
  masterWallet: WalletContext,
  receiverAddress: UnshieldedAddress,
  amount: bigint,
): Promise<string> {
  const ttl = new Date(Date.now() + 30 * 60 * 1000);

  const recipe = await masterWallet.wallet.transferTransaction(
    [{
      type: 'unshielded',
      outputs: [{
        type: ledger.nativeToken().raw,
        receiverAddress,
        amount,
      }],
    }],
    {
      shieldedSecretKeys: masterWallet.shieldedSecretKeys,
      dustSecretKey: masterWallet.dustSecretKey,
    },
    { ttl },
  );

  const signed = await masterWallet.wallet.signRecipe(
    recipe,
    (payload) => masterWallet.unshieldedKeystore.signData(payload),
  );

  const finalized = await masterWallet.wallet.finalizeRecipe(signed);
  return await masterWallet.wallet.submitTransaction(finalized);
}

/**
 * Option 1: Fund accounts from a JSON config file.
 * Each account gets 50,000 NIGHT + DUST registration.
 */
export async function fundFromConfigFile(
  masterWallet: WalletContext,
  configPath: string,
  config: Config,
): Promise<FundedAccount[]> {
  const resolved = path.resolve(configPath);
  const projectRoot = path.resolve(process.cwd());
  if (!resolved.startsWith(projectRoot + path.sep) && resolved !== projectRoot) {
    throw new Error(`Config path must be within the project directory: ${projectRoot}`);
  }

  const raw = await fs.readFile(resolved, 'utf-8');
  const accountsFile: AccountsFile = JSON.parse(raw);

  if (!accountsFile.accounts || !Array.isArray(accountsFile.accounts)) {
    throw new Error('Invalid config file: must have an "accounts" array');
  }

  if (accountsFile.accounts.length > MAX_ACCOUNTS) {
    throw new Error(`Too many accounts: max ${MAX_ACCOUNTS}, got ${accountsFile.accounts.length}`);
  }

  logger.info(`Funding ${accountsFile.accounts.length} accounts from config file...`);
  const funded: FundedAccount[] = [];

  for (let i = 0; i < accountsFile.accounts.length; i++) {
    const account = accountsFile.accounts[i];
    logger.info(`\n--- Account ${i + 1}/${accountsFile.accounts.length}: ${account.name} ---`);

    // Derive wallet from mnemonic to get address
    const seed = await mnemonicToSeed(account.mnemonic);
    const recipientWallet = await initWalletWithSeed(seed, config);
    const recipientAddress = await recipientWallet.wallet.unshielded.getAddress();
    logger.info(`Recipient address: ${recipientWallet.unshieldedKeystore.getBech32Address().asString()}`);

    // Transfer NIGHT from master
    logger.info(`Transferring ${NIGHT_AMOUNT} NIGHT to ${account.name}...`);
    const txId = await transferNight(masterWallet, recipientAddress, NIGHT_AMOUNT);
    logger.info(`Transfer submitted: ${txId}`);

    // Wait for recipient wallet to sync and see funds
    logger.info('Waiting for recipient wallet to sync...');
    await waitForSync(recipientWallet.wallet);
    await waitForFunds(recipientWallet.wallet);
    const balances = await displayWalletBalances(recipientWallet, config);

    // Register DUST for recipient
    logger.info(`Registering DUST for ${account.name}...`);
    await registerNightForDust(recipientWallet);

    // Capture final balances after DUST registration
    const finalBalances = await displayWalletBalances(recipientWallet, config);

    // Close recipient wallet
    await closeWallet(recipientWallet);
    logger.info(`Account ${account.name} funded and DUST registered.`);

    funded.push({
      name: account.name,
      unshieldedAddr: balances.unshieldedAddr,
      shieldedAddr: balances.shieldedAddr,
      dustAddr: balances.dustAddr,
      nightBalance: finalBalances.unshielded + finalBalances.shielded,
      dustBalance: finalBalances.dust,
    });
  }

  logger.info(`\nAll ${accountsFile.accounts.length} accounts funded with NIGHT + DUST.`);
  return funded;
}

/**
 * Option 2: Fund accounts by public key (Bech32 address).
 * Each account gets 50,000 NIGHT only (no DUST registration).
 */
export async function fundFromPublicKeys(
  masterWallet: WalletContext,
  pubKeysInput: string,
  config: Config,
): Promise<FundedAccount[]> {
  const addressStrings = pubKeysInput
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);

  if (addressStrings.length === 0) {
    throw new Error('No addresses provided');
  }

  if (addressStrings.length > MAX_ACCOUNTS) {
    throw new Error(`Too many addresses: max ${MAX_ACCOUNTS}, got ${addressStrings.length}`);
  }

  logger.info(`Funding ${addressStrings.length} addresses with NIGHT only...`);
  const funded: FundedAccount[] = [];

  for (let i = 0; i < addressStrings.length; i++) {
    const addressStr = addressStrings[i];
    logger.info(`\n--- Address ${i + 1}/${addressStrings.length}: ${addressStr} ---`);

    const parsed = MidnightBech32m.parse(addressStr);
    // networkId type mismatch between Config (string) and codec (branded type)
    const unshieldedAddress = UnshieldedAddress.codec.decode(config.networkId as Parameters<typeof UnshieldedAddress.codec.decode>[0], parsed);

    logger.info(`Transferring ${NIGHT_AMOUNT} NIGHT...`);
    const txId = await transferNight(masterWallet, unshieldedAddress, NIGHT_AMOUNT);
    logger.info(`Transfer submitted: ${txId}`);

    funded.push({
      name: addressStr,
      unshieldedAddr: addressStr,
      shieldedAddr: 'N/A',
      dustAddr: 'N/A',
      nightBalance: NIGHT_AMOUNT,
      dustBalance: 0n,
    });
  }

  logger.info(`\nAll ${addressStrings.length} addresses funded with NIGHT (DUST not registered - recipients must do it themselves).`);
  return funded;
}

/**
 * Transfer NIGHT tokens from master wallet directly to a shielded receiver address.
 */
async function transferShieldedNight(
  masterWallet: WalletContext,
  receiverAddress: ShieldedAddress,
  amount: bigint,
): Promise<string> {
  const ttl = new Date(Date.now() + 30 * 60 * 1000);

  const recipe = await masterWallet.wallet.transferTransaction(
    [{
      type: 'shielded',
      outputs: [{
        type: ledger.nativeToken().raw,
        receiverAddress,
        amount,
      }],
    }],
    {
      shieldedSecretKeys: masterWallet.shieldedSecretKeys,
      dustSecretKey: masterWallet.dustSecretKey,
    },
    { ttl },
  );

  const signed = await masterWallet.wallet.signRecipe(
    recipe,
    (payload) => masterWallet.unshieldedKeystore.signData(payload),
  );

  const finalized = await masterWallet.wallet.finalizeRecipe(signed);
  return await masterWallet.wallet.submitTransaction(finalized);
}

/**
 * Option 3: Fund shielded addresses directly by shielded Bech32 address.
 * Each address receives 50,000 NIGHT into its shielded balance.
 * Note: DUST registration is not possible without a mnemonic; the wallet owner
 * must register for DUST themselves using their own wallet.
 */
export async function fundShieldedAddresses(
  masterWallet: WalletContext,
  addressesInput: string,
  config: Config,
): Promise<FundedAccount[]> {
  const addressStrings = addressesInput
    .split(',')
    .map((s) => s.trim())
    .filter((s) => s.length > 0);

  if (addressStrings.length === 0) {
    throw new Error('No addresses provided');
  }

  if (addressStrings.length > MAX_ACCOUNTS) {
    throw new Error(`Too many addresses: max ${MAX_ACCOUNTS}, got ${addressStrings.length}`);
  }

  logger.info(`Funding ${addressStrings.length} shielded addresses with NIGHT...`);
  const funded: FundedAccount[] = [];

  for (let i = 0; i < addressStrings.length; i++) {
    const addressStr = addressStrings[i];
    logger.info(`\n--- Shielded Address ${i + 1}/${addressStrings.length}: ${addressStr} ---`);

    const parsed = MidnightBech32m.parse(addressStr);
    const shieldedAddress = ShieldedAddress.codec.decode(
      config.networkId as Parameters<typeof ShieldedAddress.codec.decode>[0],
      parsed,
    );

    logger.info(`Transferring ${NIGHT_AMOUNT} NIGHT to shielded address...`);
    const txId = await transferShieldedNight(masterWallet, shieldedAddress, NIGHT_AMOUNT);
    logger.info(`Transfer submitted: ${txId}`);

    funded.push({
      name: addressStr,
      unshieldedAddr: 'N/A',
      shieldedAddr: addressStr,
      dustAddr: 'N/A',
      nightBalance: NIGHT_AMOUNT,
      dustBalance: 0n,
    });
  }

  logger.info(`\nAll ${addressStrings.length} shielded addresses funded with NIGHT.`);
  return funded;
}

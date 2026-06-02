import { spawn } from 'node:child_process';
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
import { type Config, currentDir } from './config.js';

const NIGHT_AMOUNT = 50_000n * 10n ** 6n; // 50,000 NIGHT in smallest unit
const MAX_ACCOUNTS = 10;
const SPLIT_PROVE_DEFAULT_ACCOUNTS_FILE = './accounts.json';
const SPLIT_PROVE_DEFAULT_SHIELDED_ADDRESS =
  'mn_shield-addr_undeployed19jm7g77mtwmtrxj3p87gr7x9u7nup8t3ffqdww47npw0p2676j402vdmzu55upv4fs3xa8rmz8d9985ayuy2regl00hujxzad8ktzfgpnr7m7';
const SPLIT_PROVE_DEFAULT_SHIELDED_AMOUNT = NIGHT_AMOUNT;
const SPLIT_PROVE_SHIELDED_TRANSFER_ATTEMPTS = 3;
const RETRY_DELAY_MS = 5_000;

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

const sleep = (ms: number): Promise<void> => new Promise((resolve) => setTimeout(resolve, ms));

function formatNightAmount(amount: bigint): string {
  const divisor = 10n ** 6n;
  const whole = amount / divisor;
  const fractional = amount % divisor;
  if (fractional === 0n) {
    return `${whole} NIGHT (${amount} raw)`;
  }
  return `${whole}.${fractional.toString().padStart(6, '0').replace(/0+$/, '')} NIGHT (${amount} raw)`;
}

function envBigInt(name: string, defaultValue: bigint): bigint {
  const raw = process.env[name]?.trim();
  if (!raw) {
    return defaultValue;
  }
  if (!/^[0-9]+$/.test(raw)) {
    throw new Error(`${name} must be a positive integer in smallest NIGHT units`);
  }
  const value = BigInt(raw);
  if (value === 0n) {
    throw new Error(`${name} must be greater than zero`);
  }
  return value;
}

async function retryTransfer<T>(
  label: string,
  attempts: number,
  transfer: () => Promise<T>,
): Promise<T> {
  let lastError: unknown;

  for (let attempt = 1; attempt <= attempts; attempt++) {
    try {
      if (attempt > 1) {
        logger.info(`${label}: retrying attempt ${attempt}/${attempts}...`);
      }
      return await transfer();
    } catch (e) {
      lastError = e;
      const message = e instanceof Error ? e.message : String(e);
      if (attempt >= attempts) {
        break;
      }

      logger.warn(`${label}: attempt ${attempt}/${attempts} failed: ${message}`);
      logger.warn(`Waiting ${RETRY_DELAY_MS / 1000}s for wallet/proof-server state to settle before retrying...`);
      await sleep(RETRY_DELAY_MS);
    }
  }

  throw lastError;
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
    const txId = await transferShieldedNightWithRetry(masterWallet, shieldedAddress, NIGHT_AMOUNT, 1);
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

async function transferShieldedNightWithRetry(
  masterWallet: WalletContext,
  receiverAddress: ShieldedAddress,
  amount: bigint,
  attempts: number,
): Promise<string> {
  return retryTransfer('Shielded NIGHT transfer', attempts, async () => {
    await waitForSync(masterWallet.wallet);
    return transferShieldedNight(masterWallet, receiverAddress, amount);
  });
}

/**
 * Register the split-prove wallet's reg_leaf on-chain by shelling out to
 * `make register-wallet` at the repo root (builds + runs the
 * `local-poc-register-wallet` binary with `.env` sourced). The registry lives in
 * on-chain state, so registration must be redone on every fresh chain — i.e.
 * after each `make local-nodes` — exactly like funding. Folding it into the
 * "Prepare split-prove e2e funding" step keeps the e2e prerequisites in one
 * place. The binary is idempotent (re-running prints `already_registered`).
 *
 * Set MIDNIGHT_SPLIT_PROVE_SKIP_REGISTER=1 to skip (e.g. to fund without the
 * cargo build / on-chain register tx).
 */
async function registerSplitProveWallet(): Promise<void> {
  // currentDir = deps/midnight-local-dev/src → repo root is three levels up.
  const repoRoot = path.resolve(currentDir, '..', '..', '..');
  logger.info('Registering split-prove wallet on-chain (`make register-wallet`)...');
  await new Promise<void>((resolvePromise, reject) => {
    const child = spawn('make', ['register-wallet'], {
      cwd: repoRoot,
      stdio: 'inherit',
      env: process.env,
    });
    child.on('error', reject);
    child.on('close', (code) => {
      if (code === 0) {
        resolvePromise();
      } else {
        reject(new Error(`make register-wallet exited with code ${code}`));
      }
    });
  });
  logger.info('Split-prove wallet registration complete.');
}

export async function fundSplitProveE2ESetup(
  masterWallet: WalletContext,
  config: Config,
): Promise<FundedAccount[]> {
  const accountsFile = process.env.MIDNIGHT_SPLIT_PROVE_ACCOUNTS_FILE || SPLIT_PROVE_DEFAULT_ACCOUNTS_FILE;
  const shieldedAddressInput =
    process.env.MIDNIGHT_SPLIT_PROVE_SHIELDED_ADDRESS || SPLIT_PROVE_DEFAULT_SHIELDED_ADDRESS;
  const shieldedAmount = envBigInt(
    'MIDNIGHT_SPLIT_PROVE_SHIELDED_AMOUNT',
    SPLIT_PROVE_DEFAULT_SHIELDED_AMOUNT,
  );

  logger.info('Preparing split-prove e2e: funding config accounts + shielded coin, then on-chain registration...');
  logger.info(`Accounts file: ${accountsFile}`);
  logger.info(`Split-prove shielded address: ${shieldedAddressInput}`);
  logger.info(`Split-prove shielded amount: ${formatNightAmount(shieldedAmount)}`);

  const funded = await fundFromConfigFile(masterWallet, accountsFile, config);

  const parsed = MidnightBech32m.parse(shieldedAddressInput);
  const shieldedAddress = ShieldedAddress.codec.decode(
    config.networkId as Parameters<typeof ShieldedAddress.codec.decode>[0],
    parsed,
  );

  logger.info(`Funding split-prove shielded wallet with ${formatNightAmount(shieldedAmount)} and retry...`);
  const txId = await transferShieldedNightWithRetry(
    masterWallet,
    shieldedAddress,
    shieldedAmount,
    SPLIT_PROVE_SHIELDED_TRANSFER_ATTEMPTS,
  );
  logger.info(`Split-prove shielded transfer submitted: ${txId}`);

  funded.push({
    name: 'split-prove-e2e-shielded',
    unshieldedAddr: 'N/A',
    shieldedAddr: shieldedAddressInput,
    dustAddr: 'N/A',
    nightBalance: shieldedAmount,
    dustBalance: 0n,
  });

  if (process.env.MIDNIGHT_SPLIT_PROVE_SKIP_REGISTER === '1') {
    logger.info('Skipping on-chain registration (MIDNIGHT_SPLIT_PROVE_SKIP_REGISTER=1).');
    logger.info('Run `make register-wallet` before `make e2e`.');
  } else {
    try {
      await registerSplitProveWallet();
    } catch (e) {
      logger.error(`Wallet registration failed: ${e instanceof Error ? e.message : e}`);
      logger.error('Run `make register-wallet` manually before `make e2e`.');
    }
  }

  logger.info('Split-prove e2e local funding complete.');
  return funded;
}

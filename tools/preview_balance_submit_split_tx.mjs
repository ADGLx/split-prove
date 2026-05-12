#!/usr/bin/env node
import { pathToFileURL } from 'node:url';
import { existsSync } from 'node:fs';
import { mnemonicToSeedSync } from '@scure/bip39';

const DEFAULT_PREVIEW = {
  networkId: 'preview',
  indexerHttpUrl: 'https://indexer.preview.midnight.network/api/v4/graphql',
  indexerWsUrl: 'wss://indexer.preview.midnight.network/api/v4/graphql/ws',
  nodeUrl: 'wss://rpc.preview.midnight.network',
};

function bytesToHex(bytes) {
  return Buffer.from(bytes).toString('hex');
}

function fromHex(hex) {
  const clean = String(hex ?? '').trim().replace(/^0x/i, '');
  if (!clean || clean.length % 2 !== 0 || !/^[0-9a-f]*$/i.test(clean)) {
    throw new Error('tx hex must be non-empty even-length hex');
  }
  return new Uint8Array(Buffer.from(clean, 'hex'));
}

function scaleCompactEncode(value) {
  if (!Number.isSafeInteger(value) || value < 0) {
    throw new Error(`invalid compact-encoded value: ${value}`);
  }
  if (value <= 0x3f) {
    return new Uint8Array([(value << 2) | 0x00]);
  }
  if (value <= 0x3fff) {
    const compact = (value << 2) | 0x01;
    return new Uint8Array([compact & 0xff, (compact >> 8) & 0xff]);
  }
  if (value <= 0x3fffffff) {
    const compact = (value << 2) | 0x02;
    return new Uint8Array([
      compact & 0xff,
      (compact >> 8) & 0xff,
      (compact >> 16) & 0xff,
      (compact >> 24) & 0xff,
    ]);
  }
  throw new Error(`value too large for compact encoding: ${value}`);
}

function wrapAsMidnightExtrinsic(txBytes) {
  const palletIndex = 5;
  const callIndex = 0;
  const bytesLen = scaleCompactEncode(txBytes.length);
  const callData = new Uint8Array(2 + bytesLen.length + txBytes.length);
  callData[0] = palletIndex;
  callData[1] = callIndex;
  callData.set(bytesLen, 2);
  callData.set(txBytes, 2 + bytesLen.length);

  const extrinsic = new Uint8Array(1 + callData.length);
  extrinsic[0] = 0x04;
  extrinsic.set(callData, 1);

  const extrinsicLen = scaleCompactEncode(extrinsic.length);
  const full = new Uint8Array(extrinsicLen.length + extrinsic.length);
  full.set(extrinsicLen, 0);
  full.set(extrinsic, extrinsicLen.length);
  return `0x${bytesToHex(full)}`;
}

function parseArgs(argv) {
  const args = {
    txHex: process.env.MIDNIGHT_PREVIEW_TX_HEX ?? '',
    keyIndex: Number.parseInt(process.env.MIDNIGHT_PREVIEW_ZSWAP_KEY_INDEX ?? '0', 10),
    submit: false,
    dryRun: false,
    proofServerUrl: process.env.MIDNIGHT_PROOF_SERVER_URL ?? process.env.MIDNIGHT_PREVIEW_PROOF_SERVER_URL ?? '',
  };

  for (let i = 2; i < argv.length; i += 1) {
    const arg = argv[i];
    if (arg === '--tx-hex') args.txHex = argv[++i] ?? '';
    else if (arg === '--key-index') args.keyIndex = Number.parseInt(argv[++i] ?? '', 10);
    else if (arg === '--proof-server-url') args.proofServerUrl = argv[++i] ?? '';
    else if (arg === '--submit') args.submit = true;
    else if (arg === '--dry-run') args.dryRun = true;
    else if (arg === '--help' || arg === '-h') {
      process.stdout.write(`usage: preview_balance_submit_split_tx.mjs --tx-hex HEX [--key-index N] [--proof-server-url URL] [--dry-run] [--submit]\n`);
      process.exit(0);
    } else {
      throw new Error(`unknown argument: ${arg}`);
    }
  }

  if (!Number.isSafeInteger(args.keyIndex) || args.keyIndex < 0) {
    throw new Error('--key-index must be a non-negative integer');
  }
  return args;
}

function siblingWalletNodeModulesUrl() {
  if (process.env.MIDNIGHT_PREVIEW_WALLET_NODE_MODULES) {
    const value = process.env.MIDNIGHT_PREVIEW_WALLET_NODE_MODULES;
    const suffix = value.endsWith('/') ? value : `${value}/`;
    return pathToFileURL(suffix).href;
  }
  return new URL('../../one-am-wallet/node_modules/', import.meta.url).href;
}

async function importPackage(name, fallbackPath) {
  try {
    return await import(name);
  } catch (directError) {
    const fallbackUrl = new URL(fallbackPath, siblingWalletNodeModulesUrl());
    if (!existsSync(fallbackUrl)) {
      throw new Error(
        `unable to import ${name}; install it locally or set MIDNIGHT_PREVIEW_WALLET_NODE_MODULES to a wallet node_modules directory. Original error: ${directError.message}`,
      );
    }
    return await import(fallbackUrl.href);
  }
}

function normalizeIdentifier(value) {
  if (value === undefined || value === null) return '';
  if (typeof value === 'string') return value.replace(/^0x/i, '');
  if (value instanceof Uint8Array) return bytesToHex(value);
  if (Array.isArray(value)) return bytesToHex(Uint8Array.from(value));
  if (value.buffer instanceof ArrayBuffer) return bytesToHex(new Uint8Array(value.buffer));
  return String(value).replace(/^0x/i, '');
}

function collectIdentifiers(tx) {
  try {
    return Array.from(tx.identifiers()).map(normalizeIdentifier).filter(Boolean);
  } catch {
    return [];
  }
}

function txHash(tx) {
  try {
    return normalizeIdentifier(tx.transactionHash());
  } catch {
    return '';
  }
}

function envValue(name, fallback = '') {
  const value = process.env[name];
  return value && value.trim() ? value.trim() : fallback;
}

function errorDetails(error) {
  const seen = new Set();
  const parts = [];
  let current = error;
  while (current && !seen.has(current)) {
    seen.add(current);
    if (current instanceof Error) {
      parts.push(current.message);
      current = current.cause;
    } else if (typeof current === 'object') {
      const message = current.message ?? current.reason ?? current._tag ?? JSON.stringify(current);
      parts.push(String(message));
      current = current.cause;
    } else {
      parts.push(String(current));
      break;
    }
  }
  return parts.filter(Boolean).join(' | ');
}

function debug(message) {
  if (process.env.MIDNIGHT_PREVIEW_NODE_HELPER_DEBUG) {
    process.stderr.write(`[preview-helper] ${message}\n`);
  }
}

async function retryAsync(label, attempts, delayMs, fn) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await fn(attempt);
    } catch (error) {
      lastError = error;
      if (attempt >= attempts) break;
      process.stderr.write(`${label} failed on attempt ${attempt}/${attempts}: ${errorDetails(error)}\n`);
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }
  throw lastError;
}

function deriveKeys({ HDWallet, Roles, validateMnemonic, ledger, createKeystore, PublicKey }, keyIndex) {
  const phrase = envValue('MIDNIGHT_PREVIEW_RECOVERY_PHRASE').replace(/\s+/g, ' ');
  if (!phrase) {
    throw new Error('MIDNIGHT_PREVIEW_RECOVERY_PHRASE is required for wallet submit mode');
  }
  if (typeof validateMnemonic === 'function' && !validateMnemonic(phrase)) {
    throw new Error('MIDNIGHT_PREVIEW_RECOVERY_PHRASE is not a valid English BIP39 mnemonic');
  }

  const account = Number.parseInt(envValue('MIDNIGHT_PREVIEW_ACCOUNT', '0'), 10);
  if (!Number.isSafeInteger(account) || account < 0) {
    throw new Error('MIDNIGHT_PREVIEW_ACCOUNT must be a non-negative integer');
  }

  const seed = mnemonicToSeedSync(phrase);
  const wallet = HDWallet.fromSeed(seed);
  if (wallet.type !== 'seedOk') {
    throw new Error(`HD wallet seed derivation failed: ${wallet.error}`);
  }

  const derivation = wallet.hdWallet
    .selectAccount(account)
    .selectRoles([Roles.Zswap, Roles.NightExternal, Roles.Dust])
    .deriveKeysAt(keyIndex);
  wallet.hdWallet.clear();
  if (derivation.type !== 'keysDerived') {
    throw new Error('wallet key derivation failed');
  }

  const zswapSecretKeys = ledger.ZswapSecretKeys.fromSeed(derivation.keys[Roles.Zswap]);
  const dustSecretKey = ledger.DustSecretKey.fromSeed(derivation.keys[Roles.Dust]);
  const nightExternalKey = derivation.keys[Roles.NightExternal];
  const unshieldedKeystore = createKeystore(nightExternalKey, envValue('MIDNIGHT_PREVIEW_NETWORK_ID', DEFAULT_PREVIEW.networkId));
  return {
    zswapSecretKeys,
    dustSecretKey,
    unshieldedKeystore,
    publicKey: PublicKey.fromKeyStore(unshieldedKeystore),
  };
}

function createNetworkConfiguration({ InMemoryTransactionHistoryStorage, WalletEntrySchema, mergeWalletEntries }) {
  const indexerHttpUrl = envValue('MIDNIGHT_PREVIEW_INDEXER_HTTP', DEFAULT_PREVIEW.indexerHttpUrl);
  const indexerWsUrl = envValue('MIDNIGHT_PREVIEW_INDEXER_WS', DEFAULT_PREVIEW.indexerWsUrl);
  return {
    indexerUrl: indexerWsUrl,
    networkId: envValue('MIDNIGHT_PREVIEW_NETWORK_ID', DEFAULT_PREVIEW.networkId),
    relayURL: new URL(envValue('MIDNIGHT_PREVIEW_NODE_WS', DEFAULT_PREVIEW.nodeUrl)),
    costParameters: {
      additionalFeeOverhead: 300_000_000_000_000n,
      feeBlocksMargin: 5,
    },
    indexerClientConnection: {
      indexerHttpUrl,
      indexerWsUrl,
    },
    txHistoryStorage: new InMemoryTransactionHistoryStorage(WalletEntrySchema, mergeWalletEntries),
  };
}

async function withTimeout(promise, ms, label) {
  let timer;
  const timeout = new Promise((_, reject) => {
    timer = setTimeout(() => reject(new Error(`${label} timed out after ${ms}ms`)), ms);
  });
  try {
    return await Promise.race([promise, timeout]);
  } finally {
    clearTimeout(timer);
  }
}

function submitMode() {
  return envValue('MIDNIGHT_PREVIEW_WALLET_SUBMIT_MODE', 'sdk').toLowerCase();
}

function assertNodeUrlAllowed(nodeUrl) {
  const parsed = new URL(nodeUrl);
  const isLocalhost = parsed.hostname === 'localhost' || parsed.hostname === '127.0.0.1' || parsed.hostname === '[::1]' || parsed.hostname === '::1';
  if (parsed.protocol !== 'wss:' && !(isLocalhost && parsed.protocol === 'ws:')) {
    throw new Error('MIDNIGHT_PREVIEW_NODE_WS must use wss:// for non-localhost networks');
  }
}

async function submitFinalizedTxToNode(txHex) {
  const nodeUrl = envValue('MIDNIGHT_PREVIEW_NODE_WS', DEFAULT_PREVIEW.nodeUrl);
  assertNodeUrlAllowed(nodeUrl);
  const extrinsicHex = wrapAsMidnightExtrinsic(fromHex(txHex));
  const timeoutMs = Number.parseInt(envValue('MIDNIGHT_PREVIEW_RAW_RPC_SUBMIT_TIMEOUT_SECS', '60'), 10) * 1000;
  const boundedTimeoutMs = Number.isSafeInteger(timeoutMs) && timeoutMs > 0 ? timeoutMs : 60_000;

  return await withTimeout(new Promise((resolve, reject) => {
    const ws = new WebSocket(nodeUrl);
    ws.onopen = () => {
      debug(`raw RPC submit opened ${nodeUrl}; extrinsic_hex_len=${extrinsicHex.length - 2}`);
      ws.send(JSON.stringify({
        jsonrpc: '2.0',
        id: 1,
        method: 'author_submitExtrinsic',
        params: [extrinsicHex],
      }));
    };
    ws.onmessage = (event) => {
      try {
        if (typeof event.data !== 'string') {
          throw new Error('unsupported RPC response');
        }
        const data = JSON.parse(event.data);
        if (data.error) {
          const message = typeof data.error.message === 'string' ? data.error.message : 'RPC error';
          const detail = typeof data.error.data === 'string' ? data.error.data : '';
          reject(new Error(detail ? `${message}: ${detail}` : message));
        } else {
          debug(`raw RPC submit accepted result=${normalizeIdentifier(data.result)}`);
          resolve(normalizeIdentifier(data.result));
        }
      } catch (error) {
        reject(error instanceof Error ? error : new Error(String(error)));
      } finally {
        try {
          ws.close();
        } catch {
          // already closed
        }
      }
    };
    ws.onerror = (event) => {
      reject(new Error(event?.message || 'websocket error'));
      try {
        ws.close();
      } catch {
        // already closed
      }
    };
  }), boundedTimeoutMs, 'raw RPC transaction submit');
}

async function submitWithWalletSdk(wallet, tx) {
  const submitTimeoutMs = Number.parseInt(
    envValue('MIDNIGHT_PREVIEW_WALLET_SUBMIT_TIMEOUT_SECS', '180'),
    10,
  ) * 1000;
  return normalizeIdentifier(await withTimeout(
    wallet.submitTransaction(tx),
    Number.isSafeInteger(submitTimeoutMs) && submitTimeoutMs > 0 ? submitTimeoutMs : 180_000,
    'wallet transaction submit',
  ));
}

async function main() {
  const args = parseArgs(process.argv);
  const ledger = await importPackage('@midnight-ntwrk/ledger-v8', '@midnight-ntwrk/ledger-v8/midnight_ledger_wasm_fs.js');
  const tx = ledger.Transaction.deserialize('signature', 'proof', 'binding', fromHex(args.txHex));

  if (args.dryRun) {
    process.stdout.write(`${JSON.stringify({
      dryRun: true,
      txHash: txHash(tx),
      txIdentifiers: collectIdentifiers(tx),
      transactionHexLen: String(args.txHex).replace(/^0x/i, '').length,
    })}\n`);
    return;
  }

  const [
    facade,
    hd,
    shieldedWallet,
    unshieldedWallet,
    dustWallet,
    abstractions,
  ] = await Promise.all([
    importPackage('@midnight-ntwrk/wallet-sdk-facade', '@midnight-ntwrk/wallet-sdk-facade/dist/index.js'),
    importPackage('@midnight-ntwrk/wallet-sdk-hd', '@midnight-ntwrk/wallet-sdk-hd/dist/index.js'),
    importPackage('@midnight-ntwrk/wallet-sdk-shielded', '@midnight-ntwrk/wallet-sdk-shielded/dist/index.js'),
    importPackage('@midnight-ntwrk/wallet-sdk-unshielded-wallet', '@midnight-ntwrk/wallet-sdk-unshielded-wallet/dist/index.js'),
    importPackage('@midnight-ntwrk/wallet-sdk-dust-wallet', '@midnight-ntwrk/wallet-sdk-dust-wallet/dist/index.js'),
    importPackage('@midnight-ntwrk/wallet-sdk-abstractions', '@midnight-ntwrk/wallet-sdk-abstractions/dist/index.js'),
  ]);

  const { zswapSecretKeys, dustSecretKey, unshieldedKeystore, publicKey } = deriveKeys({
    ...hd,
    ledger,
    createKeystore: unshieldedWallet.createKeystore,
    PublicKey: unshieldedWallet.PublicKey,
  }, args.keyIndex);

  const configuration = createNetworkConfiguration({
    InMemoryTransactionHistoryStorage: abstractions.InMemoryTransactionHistoryStorage,
    WalletEntrySchema: facade.WalletEntrySchema,
    mergeWalletEntries: facade.mergeWalletEntries,
  });
  if (args.proofServerUrl) {
    configuration.provingServerUrl = new URL(args.proofServerUrl);
  }

  const wallet = await facade.WalletFacade.init({
    configuration,
    shielded: (config) => shieldedWallet.ShieldedWallet(config).startWithSecretKeys(zswapSecretKeys),
    unshielded: (config) => unshieldedWallet.UnshieldedWallet(config).startWithPublicKey(publicKey),
    dust: (config) => dustWallet.DustWallet(config).startWithSecretKey(
      dustSecretKey,
      ledger.LedgerParameters.initialParameters().dust,
    ),
  });

  const syncTimeoutMs = Number.parseInt(envValue('MIDNIGHT_PREVIEW_WALLET_SYNC_TIMEOUT_SECS', '180'), 10) * 1000;
  await wallet.start(zswapSecretKeys, dustSecretKey);
  debug('wallet started; waiting for synced state');
  await withTimeout(wallet.waitForSyncedState(), syncTimeoutMs, 'wallet Dust sync');
  debug('wallet synced; balancing transaction');

  const ttl = new Date(Date.now() + Number.parseInt(envValue('MIDNIGHT_PREVIEW_TX_TTL_SECS', '1800'), 10) * 1000);
  const recipe = await wallet.balanceFinalizedTransaction(
    tx,
    { shieldedSecretKeys: zswapSecretKeys, dustSecretKey },
    { ttl, tokenKindsToBalance: ['dust'] },
  );
  const proveAttempts = Number.parseInt(envValue('MIDNIGHT_PREVIEW_DUST_PROVE_ATTEMPTS', '2'), 10);
  const proveRetryDelayMs = Number.parseInt(envValue('MIDNIGHT_PREVIEW_DUST_PROVE_RETRY_DELAY_MS', '5000'), 10);
  const signedRecipe = await wallet.signRecipe(recipe, (payload) => unshieldedKeystore.signData(payload));
  debug('balanced recipe signed; finalizing Dust proofs');
  const balanced = await retryAsync(
    'wallet SDK Dust proof finalization',
    Number.isSafeInteger(proveAttempts) && proveAttempts > 0 ? proveAttempts : 2,
    Number.isSafeInteger(proveRetryDelayMs) && proveRetryDelayMs >= 0 ? proveRetryDelayMs : 5000,
    () => wallet.finalizeRecipe(signedRecipe),
  );
  const balancedBytes = balanced.serialize();
  const balancedTxHex = bytesToHex(balancedBytes);
  debug(`balanced transaction finalized; tx_hash=${txHash(balanced)} tx_hex_len=${balancedTxHex.length}`);
  const result = {
    balancedTxHex,
    txHash: txHash(balanced),
    txIdentifiers: collectIdentifiers(balanced),
    submitted: false,
    txId: null,
    submissionDiagnostic: null,
  };

  if (args.submit) {
    try {
      const mode = submitMode();
      debug(`submitting transaction with mode=${mode}`);
      if (mode === 'raw-rpc' || mode === 'raw_rpc' || mode === 'raw') {
        result.txId = await submitFinalizedTxToNode(balancedTxHex);
      } else if (mode === 'sdk' || mode === 'wallet' || mode === 'wallet-sdk') {
        result.txId = await submitWithWalletSdk(wallet, balanced);
      } else if (mode === 'sdk-then-raw-rpc' || mode === 'fallback' || mode === 'sdk-fallback') {
        try {
          result.txId = await submitWithWalletSdk(wallet, balanced);
        } catch (sdkError) {
          result.submissionDiagnostic = `wallet SDK submit failed, retrying raw RPC: ${errorDetails(sdkError)}`;
          result.txId = await submitFinalizedTxToNode(balancedTxHex);
        }
      } else {
        throw new Error(`unsupported MIDNIGHT_PREVIEW_WALLET_SUBMIT_MODE=${mode}; expected sdk, raw-rpc, or sdk-then-raw-rpc`);
      }
      result.submitted = true;
    } catch (error) {
      result.submissionDiagnostic = errorDetails(error);
      throw new Error(`wallet SDK submit failed: ${result.submissionDiagnostic}; ${JSON.stringify({
        txHash: result.txHash,
        txIdentifiers: result.txIdentifiers,
        balancedTxHexLen: balancedTxHex.length,
      })}`);
    }
  }

  process.stdout.write(`${JSON.stringify(result)}\n`, () => process.exit(0));
}

main().catch((error) => {
  process.stderr.write(`${JSON.stringify({
    error: 'WALLET_BALANCE_SUBMIT_FAILED',
    message: errorDetails(error),
    stack: process.env.MIDNIGHT_PREVIEW_NODE_HELPER_DEBUG ? error?.stack : undefined,
  })}\n`);
  process.exit(1);
});

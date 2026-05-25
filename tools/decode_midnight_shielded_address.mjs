#!/usr/bin/env node
import { existsSync } from 'node:fs';
import { pathToFileURL } from 'node:url';

function localDevNodeModulesUrl() {
  return new URL('../deps/midnight-local-dev/node_modules/', import.meta.url).href;
}

function siblingWalletNodeModulesUrl() {
  const value = envValue('MIDNIGHT_LOCAL_WALLET_NODE_MODULES');
  if (value) {
    const suffix = value.endsWith('/') ? value : `${value}/`;
    return pathToFileURL(suffix).href;
  }
  return new URL('../../one-am-wallet/node_modules/', import.meta.url).href;
}

async function importPackage(name, fallbackPath) {
  try {
    return await import(name);
  } catch (directError) {
    for (const root of [localDevNodeModulesUrl(), siblingWalletNodeModulesUrl()]) {
      const fallbackUrl = new URL(fallbackPath, root);
      if (existsSync(fallbackUrl)) {
        return await import(fallbackUrl.href);
      }
    }
    throw new Error(
      `unable to import ${name}; install it locally or set MIDNIGHT_LOCAL_WALLET_NODE_MODULES. Original error: ${directError.message}`,
    );
  }
}

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

function argValue(name, fallback = '') {
  const index = process.argv.indexOf(name);
  if (index >= 0) return process.argv[index + 1] ?? '';
  return fallback;
}

async function main() {
  const address = argValue('--address', envValue('MIDNIGHT_LOCAL_RECIPIENT_SHIELDED_ADDRESS')).trim();
  const networkId = argValue('--network-id', envValue('MIDNIGHT_LOCAL_NETWORK_ID', 'undeployed')).trim();
  if (!address) {
    throw new Error('recipient shielded address is required');
  }
  if (!networkId) {
    throw new Error('network id is required');
  }

  const { MidnightBech32m, ShieldedAddress } = await importPackage(
    '@midnight-ntwrk/wallet-sdk-address-format',
    '@midnight-ntwrk/wallet-sdk-address-format/dist/index.js',
  );
  const parsed = MidnightBech32m.parse(address);
  const decoded = ShieldedAddress.codec.decode(networkId, parsed);
  process.stdout.write(`${JSON.stringify({
    coinPublicKey: decoded.coinPublicKeyString(),
    encryptionPublicKey: decoded.encryptionPublicKeyString(),
  })}\n`);
}

main().catch((error) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exit(1);
});

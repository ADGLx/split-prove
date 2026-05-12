# Midnight Wallet SDK Migration Guide: v1.0.0 to v2.0.0

## Table of Contents

- [Overview](#overview)
- [Package Version Reference](#package-version-reference)
- [Docker Image Updates](#docker-image-updates)
- [WalletFacade Initialization (Critical)](#walletfacade-initialization-critical)
- [Unified Configuration](#unified-configuration)
- [Address Handling (Critical)](#address-handling-critical)
- [Dust Wallet Renames](#dust-wallet-renames)
- [Proving Decoupled from Wallets](#proving-decoupled-from-wallets)
- [Submission Service](#submission-service)
- [Pending Transactions Service](#pending-transactions-service)
- [Automatic Transaction Reversion](#automatic-transaction-reversion)
- [Shielded Wallet Changes](#shielded-wallet-changes)
- [Unshielded Wallet Changes](#unshielded-wallet-changes)
- [Abstractions Changes](#abstractions-changes)
- [Indexer Client Changes](#indexer-client-changes)
- [Dust Registration and Deregistration](#dust-registration-and-deregistration)
- [WASM Proving (New)](#wasm-proving-new)
- [Complete Initialization Example](#complete-initialization-example)
- [Migration Checklist](#migration-checklist)
- [Known Issues](#known-issues)

---

## Overview

The 2.0.0 release is a major architectural overhaul focused on **decoupling services** from individual wallets into standalone, facade-owned services. The three pillars of this release are:

1. **Proving extracted** into a standalone `ProvingService` - no longer a wallet-level concern
2. **Submission extracted** into a standalone `SubmissionService` - with automatic revert on failure
3. **Pending transaction monitoring** via new `PendingTransactionsService` - tracks TTL, detects failures, auto-reverts
4. **Standardized wallet APIs** - consistent naming across shielded, unshielded, and dust wallets
5. **Typed addresses** - `ShieldedAddress`, `UnshieldedAddress`, `DustAddress` replace raw strings

---

## Package Version Reference

| Package | v1.0.0 | v2.0.0 |
|---------|--------|--------|
| `@midnight-ntwrk/wallet-sdk-facade` | 1.0.0 | **2.0.0** |
| `@midnight-ntwrk/wallet-sdk-abstractions` | 1.0.0 | **2.0.0** |
| `@midnight-ntwrk/wallet-sdk-shielded` | 1.0.0 | **2.0.0** |
| `@midnight-ntwrk/wallet-sdk-dust-wallet` | 1.0.0 | **2.0.0** |
| `@midnight-ntwrk/wallet-sdk-unshielded-wallet` | 1.0.0 | **2.0.0** |
| `@midnight-ntwrk/wallet-sdk-capabilities` | _(new)_ | **3.1.0** |
| `@midnight-ntwrk/wallet-sdk-address-format` | 3.0.0 | **3.0.1** |
| `@midnight-ntwrk/wallet-sdk-hd` | 3.0.0 | **3.0.1** |
| `@midnight-ntwrk/wallet-sdk-prover-client` | _(new)_ | **1.1.0** |
| `@midnight-ntwrk/wallet-sdk-indexer-client` | _(new)_ | **1.1.0** |
| `@midnight-ntwrk/ledger-v7` | 7.0.0 | **7.0.2** |

---

## Docker Image Updates

| Image | v1.0.0 | v2.0.0 |
|-------|--------|--------|
| `midnightntwrk/midnight-node` | 0.20.0 | **0.21.0** |
| `midnightntwrk/indexer-standalone` | 3.0.0 | **3.1.0** |
| `midnightntwrk/proof-server` | 7.0.0 | **7.0.0** (unchanged) |

---

## WalletFacade Initialization (Critical)

This is the single most impactful breaking change. The `WalletFacade` constructor is now **private**. You must use the static async `WalletFacade.init()` method.

### Before (v1.0.0)

```typescript
import { WalletFacade } from '@midnight-ntwrk/wallet-sdk-facade';

// Create each wallet with its own config
const shieldedWallet = ShieldedWallet(shieldedConfig).startWithSecretKeys(shieldedSecretKeys);
const unshieldedWallet = UnshieldedWallet(unshieldedConfig).startWithPublicKey(publicKey);
const dustWallet = DustWallet(dustConfig).startWithSecretKey(dustSecretKey, dustParams);

// Pass wallets directly to constructor
const facade = new WalletFacade(shieldedWallet, unshieldedWallet, dustWallet);
await facade.start(shieldedSecretKeys, dustSecretKey);
```

### After (v2.0.0)

```typescript
import { type DefaultConfiguration, WalletFacade } from '@midnight-ntwrk/wallet-sdk-facade';

// Single unified configuration for all wallets
const configuration: DefaultConfiguration = {
  networkId: 'undeployed',
  indexerClientConnection: {
    indexerHttpUrl: 'http://localhost:8088/api/v3/graphql',
    indexerWsUrl: 'ws://localhost:8088/api/v3/graphql/ws',
  },
  provingServerUrl: new URL('http://localhost:6300'),
  relayURL: new URL('ws://localhost:9944'),
  costParameters: {
    additionalFeeOverhead: 300_000_000_000_000n,
    feeBlocksMargin: 5,
  },
  txHistoryStorage: new InMemoryTransactionHistoryStorage(),
};

// Pass wallet builder functions — config is forwarded automatically
const facade = await WalletFacade.init({
  configuration,
  shielded: (cfg) => ShieldedWallet(cfg).startWithSecretKeys(shieldedSecretKeys),
  unshielded: (cfg) => UnshieldedWallet(cfg).startWithPublicKey(publicKey),
  dust: (cfg) => DustWallet(cfg).startWithSecretKey(dustSecretKey, dustParams),
  // Optional: custom proving service (defaults to HTTP prover using provingServerUrl)
  // provingService: (cfg) => makeWasmProvingService(),
});
await facade.start(shieldedSecretKeys, dustSecretKey);
```

### Key differences

- **No more separate configs** — a single `DefaultConfiguration` is shared by all wallet builders
- **Builder functions receive config** — the `(cfg) =>` pattern lets each builder extract what it needs
- **Services are auto-created** — `SubmissionService`, `ProvingService`, and `PendingTransactionsService` are created internally by default
- **`wallet.start()` is still required** after `init()`

### DefaultConfiguration type

```typescript
type DefaultConfiguration =
  DefaultUnshieldedConfiguration &    // networkId, indexerClientConnection, txHistoryStorage
  DefaultShieldedConfiguration &      // networkId, indexerClientConnection, relayURL
  DefaultDustConfiguration &          // networkId, indexerClientConnection, costParameters, relayURL
  DefaultSubmissionConfiguration &    // relayURL
  DefaultPendingTransactionsServiceConfiguration &  // indexerClientConnection, costParameters
  Partial<DefaultProvingConfiguration>;             // provingServerUrl (optional if custom service provided)
```

---

## Unified Configuration

### Before (v1.0.0) — Three separate config objects

```typescript
const shieldedConfig = {
  networkId: config.networkId,
  indexerClientConnection: { indexerHttpUrl: config.indexer, indexerWsUrl: config.indexerWS },
  provingServerUrl: new URL(config.proofServer),
  relayURL,
};

const unshieldedConfig = {
  networkId: config.networkId,
  indexerClientConnection: { indexerHttpUrl: config.indexer, indexerWsUrl: config.indexerWS },
  txHistoryStorage: new InMemoryTransactionHistoryStorage(),
};

const dustConfig = {
  networkId: config.networkId,
  costParameters: { additionalFeeOverhead: 300_000_000_000_000n, feeBlocksMargin: 5 },
  indexerClientConnection: { indexerHttpUrl: config.indexer, indexerWsUrl: config.indexerWS },
  provingServerUrl: new URL(config.proofServer),
  relayURL,
};
```

### After (v2.0.0) — One unified configuration

```typescript
const configuration: DefaultConfiguration = {
  networkId: config.networkId,
  indexerClientConnection: {
    indexerHttpUrl: config.indexer,
    indexerWsUrl: config.indexerWS,
  },
  provingServerUrl: new URL(config.proofServer),
  relayURL: new URL(config.node.replace(/^http/, 'ws')),
  costParameters: {
    additionalFeeOverhead: 300_000_000_000_000n,
    feeBlocksMargin: 5,
  },
  txHistoryStorage: new InMemoryTransactionHistoryStorage(),
};
```

---

## Address Handling (Critical)

In v1.0.0, addresses were plain strings (Bech32-encoded). In v2.0.0, they are **typed objects**: `ShieldedAddress`, `UnshieldedAddress`, `DustAddress`.

### Transfers — Before (v1.0.0)

```typescript
// Encode address to string manually
const receiverAddress = recipientKeystore.getBech32Address().asString();

wallet.transferTransaction(
  [{
    type: 'unshielded',
    outputs: [{
      type: ledger.nativeToken().raw,
      receiverAddress,  // string
      amount: 50_000n,
    }],
  }],
  { shieldedSecretKeys, dustSecretKey },
  { ttl },
);
```

### Transfers — After (v2.0.0)

```typescript
// Get typed address from the wallet API
const receiverAddress = await recipientWallet.unshielded.getAddress();  // UnshieldedAddress

wallet.transferTransaction(
  [{
    type: 'unshielded',
    outputs: [{
      type: ledger.nativeToken().raw,
      receiverAddress,  // UnshieldedAddress (typed)
      amount: 50_000n,
    }],
  }],
  { shieldedSecretKeys, dustSecretKey },
  { ttl },
);
```

### Parsing Bech32 strings to typed addresses

If you receive an address as a Bech32 string (e.g. from user input), parse it:

```typescript
import { MidnightBech32m, UnshieldedAddress } from '@midnight-ntwrk/wallet-sdk-address-format';

const parsed = MidnightBech32m.parse('mn_addr_undeployed1...');
const unshieldedAddress = UnshieldedAddress.codec.decode(networkId, parsed);
```

### Display — encoding typed addresses to strings

For display, you can still encode typed addresses to Bech32 strings:

```typescript
import { MidnightBech32m } from '@midnight-ntwrk/wallet-sdk-address-format';

// From typed address to string
const bech32 = MidnightBech32m.encode(networkId, state.shielded.address).asString();

// Keystore still provides a convenience method for unshielded
const unshieldedStr = keystore.getBech32Address().asString();
```

### Getting addresses from wallet APIs

Each wallet type now has a `getAddress()` method:

```typescript
const shieldedAddr: ShieldedAddress = await wallet.shielded.getAddress();
const unshieldedAddr: UnshieldedAddress = await wallet.unshielded.getAddress();
const dustAddr: DustAddress = await wallet.dust.getAddress();
```

### Type definitions

```typescript
// TokenTransfer now requires typed addresses
interface TokenTransfer<AddressType extends ShieldedAddress | UnshieldedAddress> {
  type: ledger.RawTokenType;
  receiverAddress: AddressType;  // was: string
  amount: bigint;
}

type ShieldedTokenTransfer = {
  type: 'shielded';
  outputs: TokenTransfer<ShieldedAddress>[];
};

type UnshieldedTokenTransfer = {
  type: 'unshielded';
  outputs: TokenTransfer<UnshieldedAddress>[];
};

type CombinedTokenTransfer = ShieldedTokenTransfer | UnshieldedTokenTransfer;
```

---

## Dust Wallet Renames

All dust-prefixed names have been simplified for consistency across wallet types.

### State properties

| v1.0.0 | v2.0.0 |
|--------|--------|
| `state.dust.walletBalance(new Date())` | `state.dust.balance(new Date())` |
| `state.dust.dustPublicKey` | `state.dust.publicKey` |
| `state.dust.dustAddress` | `state.dust.address` |

### API methods

| v1.0.0 | v2.0.0 |
|--------|--------|
| `wallet.dust.getDustPublicKey()` | `wallet.dust.getPublicKey()` |
| `wallet.dust.getDustAddress()` | `wallet.dust.getAddress()` |
| `wallet.dust.proveTransaction()` | **Removed** — proving handled by facade |

### Types

| v1.0.0 | v2.0.0 |
|--------|--------|
| `DustCoreWallet` | `CoreWallet` |
| `dustReceiverAddress: string` | `dustReceiverAddress: DustAddress` |

### Example

```typescript
// Before
const dustBalance = state.dust?.walletBalance(new Date()) ?? 0n;
const dustPubKey = state.dust?.dustPublicKey;

// After
const dustBalance = state.dust?.balance(new Date()) ?? 0n;
const dustPubKey = state.dust?.publicKey;
```

---

## Proving Decoupled from Wallets

In v1.0.0, proving was configured per-wallet via builder methods. In v2.0.0, proving is a **facade-level service**.

### What was removed

- `ShieldedWallet(config).withProving(...)` — **removed**
- `ShieldedWallet(config).withProvingDefaults(...)` — **removed**
- `DustWallet(config).withProving(...)` — **removed**
- `DustWallet(config).withProvingDefaults(...)` — **removed**
- `finalizeTransaction()` on `ShieldedWalletAPI` — **removed**
- `proveTransaction()` on `DustWalletAPI` — **removed**
- `Proving` export from `@midnight-ntwrk/wallet-sdk-shielded/v1` — **removed**
- `provingService` parameter in V1 builders — **removed**
- `UnboundTransaction` export from facade — **moved**

### Where things moved

| What | Old location | New location |
|------|-------------|-------------|
| `ProvingService` | N/A (internal) | `@midnight-ntwrk/wallet-sdk-capabilities/proving` |
| `UnboundTransaction` | `@midnight-ntwrk/wallet-sdk-facade` | `@midnight-ntwrk/wallet-sdk-capabilities/proving` |
| `makeDefaultProvingService` | N/A | `@midnight-ntwrk/wallet-sdk-capabilities/proving` |

### How proving works now

```typescript
// DEFAULT: HTTP prover — just set provingServerUrl in configuration
const wallet = await WalletFacade.init({
  configuration: {
    ...config,
    provingServerUrl: new URL('http://localhost:6300'),  // <-- this is enough
  },
  shielded: ...,
  unshielded: ...,
  dust: ...,
  // provingService omitted → uses HTTP prover at provingServerUrl
});

// CUSTOM: provide your own proving service
import { makeWasmProvingService } from '@midnight-ntwrk/wallet-sdk-capabilities';

const wallet = await WalletFacade.init({
  configuration,  // no provingServerUrl needed
  shielded: ...,
  unshielded: ...,
  dust: ...,
  provingService: () => makeWasmProvingService(),
});
```

### Transaction proving and finalization

The `WalletFacade` now owns the `finalizeTransaction()` method, which:

1. Calls `provingService.prove(tx)` to produce an `UnboundTransaction`
2. Binds it to get a `FinalizedTransaction`
3. Registers it with `PendingTransactionsService`
4. On failure: **automatically reverts** across all three wallet types

```typescript
// The flow remains the same from the consumer perspective:
const recipe = await wallet.transferTransaction(outputs, secretKeys, options);
const signed = await wallet.signRecipe(recipe, signFn);
const finalized = await wallet.finalizeRecipe(signed);   // proving happens here
const txId = await wallet.submitTransaction(finalized);  // submission happens here
```

---

## Submission Service

Submission was extracted into a standalone service owned by the facade.

### What changed

- `WalletFacade.submitTransaction()` now delegates to an internal `SubmissionService`
- On submission failure, the facade **automatically reverts** the transaction across all wallets
- The service uses the `relayURL` from `DefaultConfiguration`

### Custom submission service (optional)

```typescript
const wallet = await WalletFacade.init({
  configuration,
  submissionService: (config) => myCustomSubmissionService,
  ...
});
```

---

## Pending Transactions Service

This is a **new service** that monitors submitted transactions.

### What it does

1. Tracks transactions after finalization
2. Monitors TTL expiration and indexer status
3. Automatically reverts wallet state when a transaction fails
4. Reports pending transaction state via `facadeState.pending`
5. State is serializable — survives wallet restarts

### Accessing pending state

```typescript
const state = await rx.firstValueFrom(wallet.state());
console.log(state.pending);  // PendingTransactions<FinalizedTransaction>
```

### Custom service (optional)

```typescript
const wallet = await WalletFacade.init({
  configuration,
  pendingTransactionsService: (config) => myCustomService,
  ...
});
```

---

## Automatic Transaction Reversion

In v2.0.0, the facade handles failure at every step:

| Failure point | v1.0.0 behavior | v2.0.0 behavior |
|--------------|----------------|----------------|
| Proving fails | Manual revert needed | Auto-reverts shielded + unshielded + dust |
| Submission fails | Manual revert needed | Auto-reverts all wallets |
| TTL expires | Not monitored | `PendingTransactionsService` detects and reverts |

This means pending coins (booked for a transaction) are automatically released when any part of the pipeline fails.

---

## Shielded Wallet Changes

### Removed

| API | Reason |
|-----|--------|
| `finalizeTransaction()` | Proving moved to facade |
| `startWithShieldedSeed()` | Renamed to `startWithSeed()` |
| `Proving` export from `/v1` | Moved to capabilities package |
| `withProving()` / `withProvingDefaults()` | Proving moved to facade |
| `provingService` in V1 builder | Proving moved to facade |

### Added

```typescript
// New method
const address: ShieldedAddress = await wallet.shielded.getAddress();
```

### Changed

- `receiverAddress` parameter: `string` → `ShieldedAddress`
- Transaction history getter now throws "not yet implemented"
- `DefaultV1Configuration` no longer includes `DefaultProvingConfiguration`

---

## Unshielded Wallet Changes

### Transaction history enhancement

Each `TransactionHistoryEntry` now includes coin data:

```typescript
interface TransactionHistoryEntry {
  // ... existing fields ...
  createdUtxos: Array<{
    value: bigint;
    owner: PublicKey;
    tokenType: RawTokenType;
    intentHash: string;
    outputIndex: number;
  }>;
  spentUtxos: Array<{
    value: bigint;
    owner: PublicKey;
    tokenType: RawTokenType;
    intentHash: string;
    outputIndex: number;
  }>;
}
```

### New method

```typescript
const address: UnshieldedAddress = await wallet.unshielded.getAddress();
```

### Bug fix

- `rollbackSpendByUtxo` now handles missing UTXOs gracefully (returns state unchanged instead of throwing)

---

## Abstractions Changes

### Moved

- `SyncProgress` moved **from** `@midnight-ntwrk/wallet-sdk-shielded/v1` **to** `@midnight-ntwrk/wallet-sdk-abstractions`

```typescript
// Before
import { SyncProgress } from '@midnight-ntwrk/wallet-sdk-shielded/v1';

// After
import { SyncProgress } from '@midnight-ntwrk/wallet-sdk-abstractions';
```

### Renamed

- `SerializedUnprovenTransaction` → `SerializedTransaction`

---

## Indexer Client Changes

### New: `keepAlive` configuration

```typescript
const configuration = {
  indexerClientConnection: {
    indexerHttpUrl: 'http://localhost:8088/api/v3/graphql',
    indexerWsUrl: 'ws://localhost:8088/api/v3/graphql/ws',
    keepAlive: 15_000,  // Optional, defaults to 15,000ms
  },
};
```

The `keepAlive` is forwarded to the underlying `graphql-ws` client. Useful for preventing service worker timer leaks.

### New: Promise-based QueryRunner

Execute GraphQL queries without Effect boilerplate:

```typescript
import { QueryRunner } from '@midnight-ntwrk/wallet-sdk-indexer-client';
```

---

## Dust Registration and Deregistration

### Registration (updated flow)

The `registerNightUtxosForDustGeneration` method now returns an `UnprovenTransactionRecipe`:

```typescript
const recipe = await wallet.registerNightUtxosForDustGeneration(
  nightUtxos,
  unshieldedKeystore.getPublicKey(),
  (payload) => unshieldedKeystore.signData(payload),
  // Optional: dustReceiverAddress (DustAddress) — defaults to own address
);

// recipe.type === 'UNPROVEN_TRANSACTION'
const finalized = await wallet.finalizeRecipe(recipe);
const txId = await wallet.submitTransaction(finalized);
```

### Deregistration (new in v2.0.0)

```typescript
const recipe = await wallet.deregisterFromDustGeneration(
  nightUtxos,
  unshieldedKeystore.getPublicKey(),
  (payload) => unshieldedKeystore.signData(payload),
);

// Balance and finalize
const balanced = await wallet.balanceUnprovenTransaction(
  recipe.transaction,
  { shieldedSecretKeys, dustSecretKey },
  { ttl: new Date(Date.now() + 30 * 60 * 1000), tokenKindsToBalance: ['dust'] },
);

const finalized = await wallet.finalizeRecipe(balanced);
await wallet.submitTransaction(finalized);
```

### Redesignation (new in v2.0.0)

Register your Night UTXOs to generate dust for a **different** wallet:

```typescript
const receiverDustAddress = receiverState.dust.address;  // DustAddress of target wallet

const recipe = await senderWallet.registerNightUtxosForDustGeneration(
  senderNightUtxos,
  senderKeystore.getPublicKey(),
  (payload) => senderKeystore.signData(payload),
  receiverDustAddress,  // <-- designate dust to another address
);

const balanced = await senderWallet.balanceUnprovenTransaction(
  recipe.transaction,
  { shieldedSecretKeys, dustSecretKey },
  { ttl, tokenKindsToBalance: ['dust'] },
);

const finalized = await senderWallet.finalizeRecipe(balanced);
await senderWallet.submitTransaction(finalized);
```

### New flag on UTXOs

```typescript
// Check if a coin is already registered
const unregistered = state.unshielded.availableCoins.filter(
  (coin) => coin.meta.registeredForDustGeneration === false,
);
```

---

## WASM Proving (New)

v2.0.0 adds WASM-based proving as an alternative to the HTTP proof server. Proofs are generated in a Web Worker.

### Setup

```typescript
import { makeWasmProvingService } from '@midnight-ntwrk/wallet-sdk-capabilities';

const wallet = await WalletFacade.init({
  configuration: {
    networkId: 'undeployed',
    relayURL: new URL('ws://localhost:9944'),
    indexerClientConnection: { ... },
    costParameters: { ... },
    txHistoryStorage: new InMemoryTransactionHistoryStorage(),
    // No provingServerUrl needed!
  },
  shielded: (cfg) => ShieldedWallet(cfg).startWithSecretKeys(shieldedSecretKeys),
  unshielded: (cfg) => UnshieldedWallet(cfg).startWithPublicKey(publicKey),
  dust: (cfg) => DustWallet(cfg).startWithSecretKey(dustSecretKey, dustParams),
  provingService: () => makeWasmProvingService(),
});
```

### Notes

- Uses Midnight-specific key material (not Filecoin keys)
- Proof generation runs in a Web Worker to avoid blocking the main thread
- No external proof server required
- Suitable for browser-based wallets

---

## Complete Initialization Example

Here is the full v2.0.0 initialization pattern, matching the official docs-snippets:

```typescript
import * as ledger from '@midnight-ntwrk/ledger-v7';
import { DustWallet } from '@midnight-ntwrk/wallet-sdk-dust-wallet';
import { type DefaultConfiguration, WalletFacade } from '@midnight-ntwrk/wallet-sdk-facade';
import { HDWallet, Roles } from '@midnight-ntwrk/wallet-sdk-hd';
import { ShieldedWallet } from '@midnight-ntwrk/wallet-sdk-shielded';
import {
  createKeystore,
  InMemoryTransactionHistoryStorage,
  PublicKey,
  UnshieldedWallet,
} from '@midnight-ntwrk/wallet-sdk-unshielded-wallet';
import { Buffer } from 'buffer';

const configuration: DefaultConfiguration = {
  networkId: 'undeployed',
  costParameters: {
    additionalFeeOverhead: 300_000_000_000_000n,
    feeBlocksMargin: 5,
  },
  relayURL: new URL('ws://localhost:9944'),
  provingServerUrl: new URL('http://localhost:6300'),
  indexerClientConnection: {
    indexerHttpUrl: 'http://localhost:8088/api/v3/graphql',
    indexerWsUrl: 'ws://localhost:8088/api/v3/graphql/ws',
  },
  txHistoryStorage: new InMemoryTransactionHistoryStorage(),
};

const initWalletWithSeed = async (seed: Buffer) => {
  const hdWallet = HDWallet.fromSeed(seed);
  if (hdWallet.type !== 'seedOk') throw new Error('Failed to initialize HDWallet');

  const derivationResult = hdWallet.hdWallet
    .selectAccount(0)
    .selectRoles([Roles.Zswap, Roles.NightExternal, Roles.Dust])
    .deriveKeysAt(0);

  if (derivationResult.type !== 'keysDerived') throw new Error('Failed to derive keys');
  hdWallet.hdWallet.clear();

  const shieldedSecretKeys = ledger.ZswapSecretKeys.fromSeed(derivationResult.keys[Roles.Zswap]);
  const dustSecretKey = ledger.DustSecretKey.fromSeed(derivationResult.keys[Roles.Dust]);
  const unshieldedKeystore = createKeystore(
    derivationResult.keys[Roles.NightExternal],
    configuration.networkId,
  );

  const wallet = await WalletFacade.init({
    configuration,
    shielded: (cfg) => ShieldedWallet(cfg).startWithSecretKeys(shieldedSecretKeys),
    unshielded: (cfg) =>
      UnshieldedWallet(cfg).startWithPublicKey(PublicKey.fromKeyStore(unshieldedKeystore)),
    dust: (cfg) =>
      DustWallet(cfg).startWithSecretKey(
        dustSecretKey,
        ledger.LedgerParameters.initialParameters().dust,
      ),
  });

  await wallet.start(shieldedSecretKeys, dustSecretKey);

  return { wallet, shieldedSecretKeys, dustSecretKey, unshieldedKeystore };
};
```

---

## Migration Checklist

### 1. Update dependencies

Update all `@midnight-ntwrk` packages in `package.json` to v2.0.0 versions (see [Package Version Reference](#package-version-reference)).

### 2. Update Docker images

Update `standalone.yml` with new image tags (see [Docker Image Updates](#docker-image-updates)).

### 3. Replace WalletFacade constructor

- [ ] Replace `new WalletFacade(shielded, unshielded, dust)` with `WalletFacade.init({ configuration, shielded, unshielded, dust })`
- [ ] Merge separate wallet configs into a single `DefaultConfiguration`
- [ ] Import `DefaultConfiguration` from `@midnight-ntwrk/wallet-sdk-facade`

### 4. Remove proving from wallet builders

- [ ] Remove `.withProving()` / `.withProvingDefaults()` calls
- [ ] Remove `provingService` from builder configs
- [ ] Optionally pass `provingService` to `WalletFacade.init()` for custom provers

### 5. Update address handling

- [ ] Replace `string` receiver addresses with typed `ShieldedAddress` / `UnshieldedAddress`
- [ ] Use `wallet.shielded.getAddress()`, `wallet.unshielded.getAddress()`, `wallet.dust.getAddress()`
- [ ] Parse Bech32 strings via `MidnightBech32m.parse()` + `codec.decode()` where needed

### 6. Fix dust wallet renames

- [ ] `walletBalance()` → `balance()`
- [ ] `dustPublicKey` → `publicKey`
- [ ] `dustAddress` → `address`
- [ ] `getDustPublicKey()` → `getPublicKey()`
- [ ] `getDustAddress()` → `getAddress()`
- [ ] `DustCoreWallet` → `CoreWallet`

### 7. Update imports

- [ ] `UnboundTransaction` — import from `@midnight-ntwrk/wallet-sdk-capabilities/proving`
- [ ] `SyncProgress` — import from `@midnight-ntwrk/wallet-sdk-abstractions`

### 8. Handle new transaction reversion

- [ ] Remove manual revert logic for proving/submission failures (facade handles it)
- [ ] Access `state.pending` for pending transaction monitoring if needed

### 9. Test

- [ ] Verify wallet sync
- [ ] Verify transfer transactions (shielded + unshielded)
- [ ] Verify dust registration
- [ ] Verify transaction finalization and submission
- [ ] Verify graceful handling of failed transactions (auto-revert)

---

## Known Issues

### Pending coins not cleared after failed shielded transaction

When submission fails for a shielded transaction, pending coins may not be automatically released in some edge cases.

**Workaround**: Restart the wallet or re-sync to clear stale pending state.

### No transaction history for shielded/dust wallets

Shielded and dust wallets do not track transaction history. Only the unshielded wallet maintains transaction records. Calling the transaction history getter on the shielded wallet will throw a "not yet implemented" error.

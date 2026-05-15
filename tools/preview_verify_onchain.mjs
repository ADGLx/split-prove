#!/usr/bin/env node
// Independent on-chain verifier for the preview split-send e2e.
//
// Reads back the block produced by submission via a fresh WS connection to the
// node, then asserts:
//   (1) the block at block_hash contains an extrinsic whose payload embeds the
//       inner Midnight ledger transaction bytes we submitted;
//   (2) the block is on the canonical chain (chain_getBlockHash(n) == block_hash);
//   (3) the finalized head has reached at least the block's number (best-effort:
//       reported but not asserted, since finalization can lag inclusion).
//
// This does NOT reuse the submitter's RPC subscription — the point is to be a
// second, independent witness that the transaction is on chain.

const DEFAULT_NODE_URL = 'ws://127.0.0.1:9944';

function envValue(name, fallback = '') {
  const value = process.env[name];
  return value && value.trim() ? value.trim() : fallback;
}

function parseArgs(argv) {
  const args = { blockHash: '', innerTxHex: '', txId: '' };
  for (let i = 2; i < argv.length; i += 1) {
    const key = argv[i];
    const value = argv[i + 1];
    if (key === '--block-hash') { args.blockHash = value; i += 1; }
    else if (key === '--inner-tx-hex') { args.innerTxHex = value; i += 1; }
    else if (key === '--tx-id') { args.txId = value; i += 1; }
  }
  if (!args.blockHash) throw new Error('--block-hash is required');
  if (!args.innerTxHex) throw new Error('--inner-tx-hex is required');
  return args;
}

function normHex(value) {
  return String(value ?? '').trim().replace(/^0x/i, '').toLowerCase();
}

function assertNodeUrlAllowed(nodeUrl) {
  const parsed = new URL(nodeUrl);
  const isLocalhost = isLocalStackUrl(parsed);
  if (parsed.protocol !== 'wss:' && !(isLocalhost && parsed.protocol === 'ws:')) {
    throw new Error('MIDNIGHT_PREVIEW_NODE_WS must use wss:// for non-localhost networks');
  }
}

function isLocalStackUrl(parsedUrl) {
  return parsedUrl.hostname === 'localhost'
    || parsedUrl.hostname === '127.0.0.1'
    || parsedUrl.hostname === '[::1]'
    || parsedUrl.hostname === '::1';
}

function allowRemotePatchedStack() {
  return ['1', 'true', 'yes'].includes(
    envValue('MIDNIGHT_ALLOW_REMOTE_PATCHED_STACK', '').trim().toLowerCase(),
  );
}

function assertPatchedStackUrlAllowed(name, value) {
  const parsed = new URL(value);
  if (!isLocalStackUrl(parsed) && !allowRemotePatchedStack()) {
    throw new Error(
      `${name} must point at the rebuilt local split-prove stack. Set MIDNIGHT_ALLOW_REMOTE_PATCHED_STACK=1 only for a known patched node/indexer pair.`,
    );
  }
}

function rpcClient(nodeUrl, timeoutMs) {
  const ws = new WebSocket(nodeUrl);
  let nextId = 1;
  const pending = new Map();
  let openResolve;
  let openReject;
  const opened = new Promise((resolve, reject) => { openResolve = resolve; openReject = reject; });

  ws.onopen = () => openResolve();
  ws.onerror = (event) => {
    const err = new Error(event?.message || 'websocket error');
    openReject(err);
    for (const { reject } of pending.values()) reject(err);
    pending.clear();
  };
  ws.onclose = () => {
    const err = new Error('node websocket closed unexpectedly');
    for (const { reject } of pending.values()) reject(err);
    pending.clear();
  };
  ws.onmessage = (event) => {
    if (typeof event.data !== 'string') return;
    let msg;
    try { msg = JSON.parse(event.data); } catch { return; }
    if (msg.id == null) return;
    const entry = pending.get(msg.id);
    if (!entry) return;
    pending.delete(msg.id);
    if (msg.error) {
      const detail = typeof msg.error.data === 'string' ? `: ${msg.error.data}` : '';
      entry.reject(new Error(`${msg.error.message || 'RPC error'}${detail}`));
    } else {
      entry.resolve(msg.result);
    }
  };

  const call = (method, params = []) => new Promise((resolve, reject) => {
    const id = nextId;
    nextId += 1;
    const timer = setTimeout(() => {
      if (pending.has(id)) {
        pending.delete(id);
        reject(new Error(`RPC ${method} timed out after ${timeoutMs}ms`));
      }
    }, timeoutMs);
    pending.set(id, {
      resolve: (value) => { clearTimeout(timer); resolve(value); },
      reject: (err) => { clearTimeout(timer); reject(err); },
    });
    ws.send(JSON.stringify({ jsonrpc: '2.0', id, method, params }));
  });

  const close = () => { try { ws.close(); } catch { /* ignore */ } };

  return { opened, call, close };
}

async function main() {
  const args = parseArgs(process.argv);
  const nodeUrl = envValue('MIDNIGHT_PREVIEW_NODE_WS', DEFAULT_NODE_URL);
  assertPatchedStackUrlAllowed('MIDNIGHT_PREVIEW_NODE_WS', nodeUrl);
  assertNodeUrlAllowed(nodeUrl);
  const timeoutMs = Number.parseInt(envValue('MIDNIGHT_PREVIEW_VERIFY_TIMEOUT_SECS', '60'), 10) * 1000;
  const callTimeoutMs = Number.isSafeInteger(timeoutMs) && timeoutMs > 0 ? timeoutMs : 60_000;

  const blockHash = args.blockHash.startsWith('0x') ? args.blockHash : `0x${args.blockHash}`;
  const innerHex = normHex(args.innerTxHex);
  if (!innerHex) throw new Error('inner-tx-hex is empty after normalization');

  const client = rpcClient(nodeUrl, callTimeoutMs);
  try {
    await client.opened;

    const signedBlock = await client.call('chain_getBlock', [blockHash]);
    if (!signedBlock || !signedBlock.block) {
      throw new Error(`chain_getBlock returned no block for ${blockHash}`);
    }
    const header = signedBlock.block.header;
    const extrinsics = signedBlock.block.extrinsics || [];
    const blockNumberHex = header.number;
    const blockNumber = Number.parseInt(String(blockNumberHex).replace(/^0x/, ''), 16);
    if (!Number.isSafeInteger(blockNumber)) {
      throw new Error(`could not parse block number from header.number=${blockNumberHex}`);
    }

    let matchedIndex = -1;
    for (let i = 0; i < extrinsics.length; i += 1) {
      if (normHex(extrinsics[i]).includes(innerHex)) {
        matchedIndex = i;
        break;
      }
    }
    if (matchedIndex < 0) {
      throw new Error(
        `inner tx bytes (${innerHex.length / 2} bytes) not found in any of ${extrinsics.length} `
        + `extrinsic(s) at block ${blockHash} (#${blockNumber})`,
      );
    }

    const canonicalHash = await client.call('chain_getBlockHash', [blockNumber]);
    if (normHex(canonicalHash) !== normHex(blockHash)) {
      throw new Error(
        `block ${blockHash} is not canonical at height ${blockNumber}; `
        + `chain_getBlockHash(${blockNumber}) = ${canonicalHash}`,
      );
    }

    const finalizedHash = await client.call('chain_getFinalizedHead', []);
    const finalizedHeader = await client.call('chain_getHeader', [finalizedHash]);
    const finalizedNumber = Number.parseInt(String(finalizedHeader.number).replace(/^0x/, ''), 16);
    const finalizedDepth = Number.isSafeInteger(finalizedNumber)
      ? finalizedNumber - blockNumber
      : null;

    const result = {
      verified: true,
      nodeUrl,
      blockHash,
      blockNumber,
      extrinsicIndex: matchedIndex,
      extrinsicCount: extrinsics.length,
      finalizedHash,
      finalizedNumber,
      finalizedDepth,
      finalized: finalizedDepth != null && finalizedDepth >= 0,
      innerTxBytes: innerHex.length / 2,
      txId: args.txId || null,
    };
    process.stdout.write(`${JSON.stringify(result)}\n`);
  } finally {
    client.close();
  }
}

main().catch((error) => {
  const message = error instanceof Error ? error.message : String(error);
  process.stderr.write(`${JSON.stringify({ verified: false, error: message })}\n`);
  process.exit(1);
});

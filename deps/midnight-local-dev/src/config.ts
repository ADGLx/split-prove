import path from 'node:path';
import { setNetworkId } from '@midnight-ntwrk/midnight-js-network-id';

export const currentDir = path.resolve(new URL(import.meta.url).pathname, '..');

export interface Config {
  readonly logDir: string;
  indexer: string;
  indexerWS: string;
  node: string;
  proofServer: string;
  readonly networkId: string;
}

export class StandaloneConfig implements Config {
  logDir = path.resolve(currentDir, '..', 'logs', `${new Date().toISOString()}.log`);
  indexer = 'http://127.0.0.1:8088/api/v3/graphql';
  indexerWS = 'ws://127.0.0.1:8088/api/v3/graphql/ws';
  node = 'http://127.0.0.1:9944';
  proofServer = 'http://127.0.0.1:6300';
  networkId = 'undeployed';
  constructor() {
    setNetworkId('undeployed');
  }
}

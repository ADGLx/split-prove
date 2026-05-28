import 'dotenv/config';
import { StandaloneConfig } from './config.js';
import { createLogger } from './logger.js';
import {
  setLogger as setWalletLogger,
  buildWalletFromHexSeed,
  registerNightForDust,
  displayWalletBalances,
  closeWallet,
} from './wallet.js';
import { setLogger as setFundingLogger, fundSplitProveE2ESetup } from './funding.js';

const GENESIS_MINT_WALLET_SEED = '0000000000000000000000000000000000000000000000000000000000000001';

async function main(): Promise<void> {
  const config = new StandaloneConfig();
  const logger = await createLogger(config.logDir);
  setWalletLogger(logger);
  setFundingLogger(logger);

  let masterWallet = null;
  try {
    logger.info('Initializing master wallet from genesis seed...');
    masterWallet = await buildWalletFromHexSeed(config, GENESIS_MINT_WALLET_SEED);

    logger.info('Registering DUST for master wallet...');
    await registerNightForDust(masterWallet);

    await displayWalletBalances(masterWallet, config);

    logger.info('Funding split-prove e2e accounts...');
    const funded = await fundSplitProveE2ESetup(masterWallet, config);
    logger.info(`Funded ${funded.length} account(s).`);
  } finally {
    if (masterWallet !== null) {
      await closeWallet(masterWallet);
    }
  }
}

main().catch((e) => {
  console.error(e);
  process.exit(1);
});

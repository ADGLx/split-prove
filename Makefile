ENV_FILE ?= .env
LOCAL_DEV_DIR ?= deps/midnight-local-dev
DEFAULT_MIDNIGHT_NODE_IMAGE ?= midnight-node:split-prove
DEFAULT_MIDNIGHT_INDEXER_IMAGE ?= split-prove/indexer-standalone:local

.PHONY: e2e e2e-full e2e-precheck deploy-registry images rebuild-images image-node image-indexer local-install local-nodes local-up local-clean local-restart local-env

# Solution A: deploy the wallet-registry contract before the e2e test runs.
# In demo mode (no MIDNIGHT_PREVIEW_REGISTRY_PREDEPLOYED_ADDRESS), this writes
# a placeholder address — see `tools/deploy_registry.sh`.
deploy-registry:
	tools/deploy_registry.sh

# Prerequisite check: when `make e2e` runs the live preview test, the local
# Midnight node must be running with the Solution A patched code. If you've
# pulled new changes, run `make images && make local-restart` first. This
# precheck does *not* attempt to detect a stale node — that diagnosis is
# left to the test itself, which surfaces "Custom error: 127" (`MalformedError::Zswap`)
# when the running node rejects the v4 bundle envelope.
e2e-precheck:
	@printf '\n--- make e2e precheck ---\n'
	@printf 'If `cargo test preview_wallet_proves_real_unspent_split_spend`\n'
	@printf 'fails with "Custom error: 127" (`MalformedError::Zswap`), the\n'
	@printf 'running local node does not have the Solution A patched code.\n'
	@printf 'Run `make e2e-full` to rebuild the docker images, restart the\n'
	@printf 'local stack, and rerun the test.\n\n'

# Fast path: assumes the docker images already include the Solution A code
# and the local stack is running. Use this for tight inner-loop development
# (no docker rebuild, no node restart).
e2e: e2e-precheck deploy-registry
	MIDNIGHT_RUN_PREVIEW_E2E=1 MIDNIGHT_PREVIEW_RAW_RPC_WAIT_FOR=finalized cargo test -p midnight-proof-server \
	    --manifest-path deps/midnight-ledger/Cargo.toml \
	    preview_wallet_proves_real_unspent_split_spend -- --nocapture

# Full path: rebuild patched docker images, restart the local stack, deploy
# the registry, and run the live preview e2e. Use this after pulling new
# changes or when the running node is on stale code.
e2e-full: images local-restart deploy-registry
	MIDNIGHT_RUN_PREVIEW_E2E=1 MIDNIGHT_PREVIEW_RAW_RPC_WAIT_FOR=finalized cargo test -p midnight-proof-server \
	    --manifest-path deps/midnight-ledger/Cargo.toml \
	    preview_wallet_proves_real_unspent_split_spend -- --nocapture

images: image-node image-indexer

rebuild-images: images

image-node:
	set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	docker build -f deps/midnight-node/Dockerfile.split-prove \
	    -t "$${MIDNIGHT_NODE_IMAGE:-$(DEFAULT_MIDNIGHT_NODE_IMAGE)}" .

image-indexer:
	set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	docker build -f Dockerfile.indexer \
	    -t "$${MIDNIGHT_INDEXER_IMAGE:-$(DEFAULT_MIDNIGHT_INDEXER_IMAGE)}" .

local-nodes: local-up

local-install:
	cd "$(LOCAL_DEV_DIR)" && npm install

local-up:
	cd "$(LOCAL_DEV_DIR)" && { \
	    set -a; \
	    [ ! -f "$(CURDIR)/$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; \
	    set +a; \
	    npm start; \
	}

local-clean:
	cd "$(LOCAL_DEV_DIR)" && npm run clean

local-restart: local-clean local-up

local-env:
	@set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	printf 'MIDNIGHT_NODE_IMAGE=%s\n' "$${MIDNIGHT_NODE_IMAGE:-$(DEFAULT_MIDNIGHT_NODE_IMAGE)}"; \
	printf 'MIDNIGHT_INDEXER_IMAGE=%s\n' "$${MIDNIGHT_INDEXER_IMAGE:-$(DEFAULT_MIDNIGHT_INDEXER_IMAGE)}"; \
	printf 'MIDNIGHT_PROOF_SERVER_IMAGE=%s\n' "$${MIDNIGHT_PROOF_SERVER_IMAGE:-midnightntwrk/proof-server:8.0.3}"

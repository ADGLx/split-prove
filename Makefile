ENV_FILE ?= .env
LOCAL_DEV_DIR ?= deps/midnight-local-dev
DEFAULT_MIDNIGHT_NODE_IMAGE ?= midnight-node:split-prove
DEFAULT_MIDNIGHT_INDEXER_IMAGE ?= split-prove/indexer-standalone:local

.PHONY: e2e deploy-registry images rebuild-images image-node image-indexer local-install local-nodes local-up local-clean local-restart local-env

# Solution A: deploy the wallet-registry contract once per fresh local stack.
# `make e2e` depends on this so `register_wallet` has a contract to talk to
# and the patched admission path can pull the historic-roots set from it.
deploy-registry:
	tools/deploy_registry.sh

e2e: deploy-registry
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

ENV_FILE ?= .env
LOCAL_DEV_DIR ?= deps/midnight-local-dev
DEFAULT_MIDNIGHT_NODE_IMAGE ?= midnight-node:split-prove
DEFAULT_MIDNIGHT_INDEXER_IMAGE ?= split-prove/indexer-standalone:local

.PHONY: e2e rebuild-images local-nodes

# Fast path: assumes the docker images already include the Solution A code
# and the local stack is running. Use this for tight inner-loop development
# (no docker rebuild, no node restart).
e2e:
	@printf '\n--- make e2e precheck ---\n'
	@printf 'If `cargo test local_wallet_proves_real_unspent_split_spend`\n'
	@printf 'fails with "Custom error: 127" (`MalformedError::Zswap`), the\n'
	@printf 'running local node does not have the Solution A patched code.\n'
	@printf 'Run `make rebuild-images`, restart `make local-nodes`, and rerun the test.\n\n'
	MIDNIGHT_RUN_LOCAL_E2E=1 MIDNIGHT_LOCAL_RAW_RPC_WAIT_FOR=finalized cargo test -p midnight-proof-server \
	    --manifest-path deps/midnight-ledger/Cargo.toml \
	    local_wallet_proves_real_unspent_split_spend -- --nocapture

rebuild-images:
	set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	docker build -f deps/midnight-node/Dockerfile.split-prove \
	    -t "$${MIDNIGHT_NODE_IMAGE:-$(DEFAULT_MIDNIGHT_NODE_IMAGE)}" .
	set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	docker build -f Dockerfile.indexer \
	    -t "$${MIDNIGHT_INDEXER_IMAGE:-$(DEFAULT_MIDNIGHT_INDEXER_IMAGE)}" .

local-nodes:
	cd "$(LOCAL_DEV_DIR)" && { \
	    set -a; \
	    [ ! -f "$(CURDIR)/$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; \
	    set +a; \
	    npm start; \
	}

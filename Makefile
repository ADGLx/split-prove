ENV_FILE ?= .env
LOCAL_DEV_DIR ?= deps/midnight-local-dev
DEFAULT_MIDNIGHT_NODE_IMAGE ?= midnight-node:split-prove
DEFAULT_MIDNIGHT_INDEXER_IMAGE ?= split-prove/indexer-standalone:local

NATIVE_DATA_DIR  ?= .native-data
NODE_BIN         ?= deps/midnight-node/target/release/midnight-node
INDEXER_BIN      ?= deps/midnight-indexer/target/release/indexer-standalone
INDEXER_CONFIG   ?= $(CURDIR)/deps/midnight-indexer/indexer-standalone/config.yaml

.PHONY: e2e images rebuild-images image-node image-indexer local-install local-nodes local-up local-clean local-restart local-env \
        build-native build-node build-indexer \
        native-node native-indexer native-proof-server native-fund native-clean

e2e:
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

# ── Native (non-Docker) build & run ─────────────────────────────────────────
# Run each service in its own terminal, then `make e2e` as normal.
# First-time: `make build-native` to compile node and indexer outside Docker.

build-native: build-node build-indexer

build-node:
	cd deps/midnight-node && cargo build --release -p midnight-node --locked

build-indexer:
	cd deps/midnight-indexer && cargo build --release -p indexer-standalone --features standalone --locked

$(NATIVE_DATA_DIR):
	mkdir -p $@

# Terminal 1
native-node: build-node | $(NATIVE_DATA_DIR)
	cd deps/midnight-node && \
	CFG_PRESET=dev \
	SIDECHAIN_BLOCK_BENEFICIARY=04bcf7ad3be7a5c790460be82a713af570f22e0f801f6659ab8e84a52be6969e \
	APPEND_ARGS="--base-path $(CURDIR)/$(NATIVE_DATA_DIR)/node" \
	$(CURDIR)/$(NODE_BIN)

# Terminal 2 — wait for the node to be ready first
native-indexer: build-indexer | $(NATIVE_DATA_DIR)
	CONFIG_FILE=$(INDEXER_CONFIG) \
	APP__APPLICATION__NETWORK_ID=undeployed \
	APP__INFRA__NODE__URL=ws://127.0.0.1:9944 \
	APP__INFRA__STORAGE__CNN_URL=$(CURDIR)/$(NATIVE_DATA_DIR)/indexer.sqlite \
	APP__INFRA__LEDGER_DB__CNN_URL=$(CURDIR)/$(NATIVE_DATA_DIR)/ledger-db.sqlite \
	APP__INFRA__SECRET=303132333435363738393031323334353637383930313233343536373839303132 \
	RUST_LOG='indexer=info,chain_indexer=info,indexer_api=info,wallet_indexer=info,indexer_common=info,fastrace_opentelemetry=off,info' \
	$(INDEXER_BIN)

# Terminal 3 — proof server still runs in Docker (pre-built image, no source here)
native-proof-server:
	set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	docker run --rm \
	    -p 127.0.0.1:6300:6300 \
	    -e RUST_BACKTRACE=full \
	    "$${MIDNIGHT_PROOF_SERVER_IMAGE:-midnightntwrk/proof-server:8.0.3}" \
	    midnight-proof-server -v

native-fund:
	cd "$(LOCAL_DEV_DIR)" && { \
	    set -a; \
	    [ ! -f "$(CURDIR)/$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; \
	    set +a; \
	    npm run fund; \
	}

native-clean:
	rm -rf $(NATIVE_DATA_DIR)

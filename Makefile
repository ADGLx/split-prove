ENV_FILE ?= .env
LOCAL_DEV_DIR ?= deps/midnight-local-dev
DEFAULT_MIDNIGHT_NODE_IMAGE ?= midnight-node:split-prove
DEFAULT_MIDNIGHT_INDEXER_IMAGE ?= split-prove/indexer-standalone:local
LEDGER_WASM_BUILDER_IMAGE ?= split-prove/ledger-wasm-builder:local
LEDGER_WASM_OUT_DIR ?= /private/tmp/split-prove-ledger-wasm-out
LEDGER_WASM_ARTIFACT ?= $(LEDGER_WASM_OUT_DIR)/midnight_ledger_wasm.wasm

.PHONY: e2e rebuild-images local-ledger-js local-nodes

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

local-ledger-js:
	docker build -f deps/midnight-ledger/ledger-wasm/Dockerfile.local-builder \
	    -t "$(LEDGER_WASM_BUILDER_IMAGE)" deps/midnight-ledger/ledger-wasm
	mkdir -p "$(LEDGER_WASM_OUT_DIR)"
	docker run --rm \
	    -v "$(CURDIR):/work:ro" \
	    -v "$(LEDGER_WASM_OUT_DIR):/out" \
	    -v split-prove-ledger-wasm-cargo-registry:/usr/local/cargo/registry \
	    -v split-prove-ledger-wasm-cargo-git:/usr/local/cargo/git \
	    -v split-prove-ledger-wasm-target:/target \
	    -w /work/deps/midnight-ledger \
	    "$(LEDGER_WASM_BUILDER_IMAGE)" \
	    sh -c 'cargo build --package midnight-ledger-wasm --target wasm32-unknown-unknown --profile wasm --target-dir /target && cp /target/wasm32-unknown-unknown/wasm/midnight_ledger_wasm.wasm /out/midnight_ledger_wasm.wasm'
	cd deps/midnight-ledger/ledger-wasm && node build-local-ledger-v8.mjs "$(LEDGER_WASM_ARTIFACT)"
	cd "$(LOCAL_DEV_DIR)" && npm install

local-nodes: local-ledger-js
	cd "$(LOCAL_DEV_DIR)" && { \
	    set -a; \
	    [ ! -f "$(CURDIR)/$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; \
	    set +a; \
	    npm start; \
	}

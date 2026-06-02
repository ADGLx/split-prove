ENV_FILE ?= .env
LOCAL_DEV_DIR ?= deps/midnight-local-dev
DEFAULT_MIDNIGHT_NODE_IMAGE ?= midnight-node:split-prove
DEFAULT_MIDNIGHT_INDEXER_IMAGE ?= split-prove/indexer-standalone:local
LEDGER_WASM_BUILDER_IMAGE ?= split-prove/ledger-wasm-builder:local
LEDGER_WASM_OUT_DIR ?= /tmp/split-prove-ledger-wasm-out
LEDGER_WASM_ARTIFACT ?= $(LEDGER_WASM_OUT_DIR)/midnight_ledger_wasm.wasm

NATIVE_DATA_DIR  ?= .native-data
NODE_BIN         ?= deps/midnight-node/target/release/midnight-node
INDEXER_BIN      ?= deps/midnight-indexer/target/release/indexer-standalone
INDEXER_CONFIG   ?= $(CURDIR)/deps/midnight-indexer/indexer-standalone/config.yaml

.PHONY: e2e rebuild-images local-install local-ledger-js local-nodes \
        build-native build-node build-indexer build-register-wallet regen-genesis \
        native-node native-indexer native-proof-server native-fund native-clean \
        register-wallet

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

# One-time install of the vendored local-dev network's npm dependencies.
# Referenced by the README / runbook setup steps. `make local-nodes` also runs
# `npm install` after (re)generating the ledger-v8 wasm package it depends on,
# so this target is just the explicit "install once" entry point.
local-install:
	cd "$(LOCAL_DEV_DIR)" && npm install

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
	    sh -c 'cargo build --package midnight-ledger-wasm --target wasm32-unknown-unknown --profile wasm --target-dir /target && cp /target/wasm32-unknown-unknown/wasm/midnight_ledger_wasm.wasm /out/midnight_ledger_wasm.wasm && rm -rf /out/pkg && wasm-bindgen /out/midnight_ledger_wasm.wasm --out-dir /out/pkg --target bundler --omit-default-module-path --weak-refs --reference-types --no-typescript'
	cd deps/midnight-ledger/ledger-wasm && node build-local-ledger-v8.mjs "$(LEDGER_WASM_ARTIFACT)" "$(LEDGER_WASM_OUT_DIR)/pkg"
	cd "$(LOCAL_DEV_DIR)" && npm install

local-nodes: local-ledger-js
	cd "$(LOCAL_DEV_DIR)" && { \
	    set -a; \
	    [ ! -f "$(CURDIR)/$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; \
	    set +a; \
	    npm start; \
	}

# ── Native (non-Docker) build & run ─────────────────────────────────────────
# Run each service in its own terminal, then `make e2e` as normal.
# First-time: `make build-native` to compile node and indexer outside Docker.

build-native: build-node build-indexer

# Regenerate genesis state when ledger-parameters-config.json changes (e.g. new
# fields added to LedgerParameters that break deserialization of the old binary).
# Writes genesis_state_undeployed.mn and genesis_block_undeployed.mn in-place.
regen-genesis:
	mkdir -p /tmp/split-prove-genesis-seeds
	printf '{\n  "wallet-seed-0": "0000000000000000000000000000000000000000000000000000000000000001",\n  "wallet-seed-1": "0000000000000000000000000000000000000000000000000000000000000002",\n  "wallet-seed-2": "0000000000000000000000000000000000000000000000000000000000000003",\n  "wallet-seed-3": "a51c86de32d0791f7cffc3bdff1abd9bb54987f0ed5effc30c936dddbb9afd9d530c8db445e4f2d3ea42a321b260e022aadf05987c9a67ec7b6b6ca1d0593ec9"\n}\n' \
	    > /tmp/split-prove-genesis-seeds/undeployed.json
	cd deps/midnight-node && \
	cargo run --release --locked -p midnight-node-toolkit -- generate-genesis \
	    --network undeployed \
	    --seeds-file /tmp/split-prove-genesis-seeds/undeployed.json \
	    --ledger-parameters-config res/dev/ledger-parameters-config.json \
	    --cnight-generates-dust-config res/dev/cnight-config.json \
	    --ics-config res/dev/ics-config.json \
	    --reserve-config res/dev/reserve-config.json \
	    --out-dir res/genesis

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

# ── Phase 2: wallet → registry contract call ────────────────────────────────
# Submits `register(reg_leaf)` to the genesis-deployed wallet_registry. Run
# once per wallet, after `make native-node` is up. Idempotent — re-running
# after the leaf is on-chain prints `status: already_registered`.

REGISTER_WALLET_BIN ?= deps/midnight-ledger/target/release/local-poc-register-wallet

build-register-wallet:
	cd deps/midnight-ledger && \
	cargo build --release -p midnight-proof-server --bin local-poc-register-wallet --locked

register-wallet: build-register-wallet
	set -a; [ ! -f "$(ENV_FILE)" ] || . "$(CURDIR)/$(ENV_FILE)"; set +a; \
	$(CURDIR)/$(REGISTER_WALLET_BIN)

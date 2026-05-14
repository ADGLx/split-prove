.PHONY: e2e

e2e:
	MIDNIGHT_RUN_PREVIEW_E2E=1 cargo test -p midnight-proof-server \
	    --manifest-path deps/midnight-ledger/Cargo.toml \
	    preview_wallet_proves_real_unspent_split_spend -- --nocapture

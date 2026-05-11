#!/bin/bash
set -e

echo "=== Midnight Circuit Builder ==="
echo ""

# Check available tools
echo "Tools:"
command -v compactc && echo "  compactc: $(compactc --version 2>&1 || echo 'available')" || echo "  compactc: NOT AVAILABLE"
command -v zkir && echo "  zkir: available" || echo "  zkir: NOT AVAILABLE"
echo ""

mkdir -p /output/zswap /output/dust

if command -v compactc &>/dev/null; then
    echo "Step 1: Compiling .compact → .zkir"
    compactc --no-communications-commitment zswap-split.compact zswap-split
    compactc --no-communications-commitment dust-split.compact dust-split
    cp zswap-split/*.zkir /output/zswap/
    cp dust-split/*.zkir /output/dust/
    echo "  OK: zkir files generated"
else
    echo "Step 1: SKIPPED (compactc not available)"
    echo "  Copying precompiled zkir files as reference..."
    cp zkir-precompiles/zswap/*.zkir /output/zswap/
    cp zkir-precompiles/dust/*.zkir /output/dust/
    echo ""
    echo "  To compile the split circuits, you need compactc."
    echo "  The .compact source files are in /work/"
    echo "  Precompiled .zkir for the ORIGINAL circuits are in /output/"
    echo ""
    echo "  Options:"
    echo "  1. Install compactc and run: compactc --no-communications-commitment zswap-split.compact zswap-split"
    echo "  2. Use nix: nix build github:midnightntwrk/compactc"
    echo "  3. Modify the .zkir JSON files manually (see /output/zswap/spend.zkir for format)"
fi

if command -v zkir &>/dev/null; then
    echo ""
    echo "Step 2: Compiling .zkir → .bzkir + keys"
    for dir in zswap dust; do
        mkdir -p /output/$dir/keys
        zkir compile-many /output/$dir /output/$dir/keys
    done
    echo "  OK: bzkir + keys generated"
else
    echo ""
    echo "Step 2: SKIPPED (zkir binary not available)"
    echo "  The zkir tool is needed to produce .bzkir and proving/verifying keys."
fi

echo ""
echo "=== Output ==="
find /output -type f | sort

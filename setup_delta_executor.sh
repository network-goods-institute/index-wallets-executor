#!/bin/bash

echo "Delta Network Executor Setup"
echo "============================"

# Step 1: Check if delta registry is configured in global cargo config
if ! grep -q "registries.delta" ~/.cargo/config.toml 2>/dev/null; then
    echo "⚠️  Delta registry not found in ~/.cargo/config.toml"
    echo "Please add the following to your ~/.cargo/config.toml:"
    echo ""
    echo "[registries.delta]"
    echo "index = \"sparse+https://crates.repyhlabs.dev/api/v1/crates/\""
    echo "credential-provider = [\"cargo:token\"]"
    echo "token = \"srxmn3d6I9L2hA7ZacwALH1epntvRqZT\""
    echo ""
    exit 1
fi

echo "✓ Delta registry configured"

# Step 2: Install delta-cli if not already installed
if ! command -v delta-cli &> /dev/null; then
    echo "Installing delta-cli..."
    cargo install --registry delta "delta-cli"
else
    echo "✓ delta-cli already installed"
fi

# Step 3: Prepare ELA data
echo "Preparing ELA transaction data..."

# Get the public key from the keypair
KEYPAIR_PATH="${KEYPAIR_PATH:-keypair.json}"
if [ ! -f "$KEYPAIR_PATH" ]; then
    echo "⚠️  Keypair file not found at $KEYPAIR_PATH"
    echo "Please generate a keypair first by running the executor"
    exit 1
fi

# You'll need to extract the public key from your keypair here
# For now, we'll create the ELA data template
cat > ela_data.json << EOF
{
    "type": "create_executor",
    "data": {
        "ela": {
            "executor": "YOUR_PUBLIC_KEY_HERE",
            "executor_operator": "YOUR_PUBLIC_KEY_HERE",
            "shard": ${SHARD_NUMBER:-1},
            "collateral": 100,
            "activation_epoch": 0,
            "deactivation_epoch": 999999,
            "lease_per_epoch": 0
        }
    }
}
EOF

echo "✓ Created ela_data.json template"
echo ""
echo "Next steps:"
echo "1. Update the 'executor' and 'executor_operator' fields in ela_data.json with your public key"
echo "2. Run the following command to submit the ELA:"
echo ""
echo "delta-cli submit --url \"http://138.197.217.21:3000\" --keypair $KEYPAIR_PATH ela_data.json"
echo ""
echo "Note: The ELA will be processed at the end of the epoch (up to 1 hour)"
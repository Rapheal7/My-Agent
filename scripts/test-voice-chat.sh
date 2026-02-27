#!/bin/bash
# Test script for voice chat setup

echo "=== My Agent Voice Chat Test ==="
echo ""

# Check if binary exists
if [ ! -f "target/release/my-agent" ]; then
    echo "Building release binary..."
    cargo build --release
fi

echo "1. Checking configuration..."
cargo run --quiet -- config --show 2>/dev/null || echo "   No config file yet (will be created on first run)"

echo ""
echo "2. Checking for HF API key..."
if cargo run --quiet -- config --show 2>/dev/null | grep -q "HuggingFace"; then
    echo "   ✓ HuggingFace API key configured"
else
    echo "   ✗ HuggingFace API key not found"
    echo "   Run: cargo run -- config --set-hf-api-key YOUR_KEY"
fi

echo ""
echo "3. Testing local HTTP server..."
timeout 5s cargo run --quiet -- serve --port 3000 &
SERVER_PID=$!
sleep 2

if curl -s http://localhost:3000 > /dev/null; then
    echo "   ✓ HTTP server responds"
else
    echo "   ✗ HTTP server not responding"
fi

kill $SERVER_PID 2>/dev/null

echo ""
echo "4. SSL Certificate check..."
if [ -f "/etc/letsencrypt/live/agent-assistant.org/fullchain.pem" ]; then
    echo "   ✓ SSL certificate found"
    echo "   Expiry: $(openssl x509 -enddate -noout -in /etc/letsencrypt/live/agent-assistant.org/fullchain.pem | cut -d= -f2)"
else
    echo "   ✗ SSL certificate not found at expected location"
    echo "   Run: sudo certbot certonly --standalone -d agent-assistant.org"
fi

echo ""
echo "=== Test Complete ==="
echo ""
echo "To start the server with HTTPS:"
echo "  sudo ./target/release/my-agent serve --https --cert /etc/letsencrypt/live/agent-assistant.org/fullchain.pem --key /etc/letsencrypt/live/agent-assistant.org/privkey.pem --port 443"

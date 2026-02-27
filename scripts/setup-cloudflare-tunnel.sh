#!/bin/bash
# Setup Cloudflare Tunnel for Mobile Voice Chat
# This provides HTTPS with real certificates - NO certificate warnings!

set -e

echo "=== Cloudflare Tunnel Setup for Voice Chat ==="
echo ""

# Check if cloudflared is installed
if ! command -v cloudflared &> /dev/null; then
    echo "📥 Installing cloudflared..."
    curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o /tmp/cloudflared
    chmod +x /tmp/cloudflared
    sudo mv /tmp/cloudflared /usr/local/bin/
    echo "✅ cloudflared installed"
else
    echo "✅ cloudflared already installed"
fi

# Create directory for cloudflared config
mkdir -p ~/.cloudflared

# Check if already logged in
if [ ! -f ~/.cloudflared/cert.pem ]; then
    echo ""
    echo "🔑 First, login to Cloudflare:"
    echo "   Run: cloudflared tunnel login"
    echo ""
    echo "   This will open a browser window. Login with your Cloudflare account."
    echo "   After login, the cert.pem file will be automatically saved."
    echo ""
    echo "   Press Enter after you've logged in..."
    read
fi

# Check if we have the cert
if [ ! -f ~/.cloudflared/cert.pem ]; then
    echo "❌ No Cloudflare credentials found. Please run 'cloudflared tunnel login' first"
    exit 1
fi

# Create tunnel
echo "Creating tunnel..."
TUNNEL_NAME="voice-chat-$(date +%s)"
cloudflared tunnel create $TUNNEL_NAME 2>&1 | tee /tmp/tunnel-create.log

# Extract tunnel ID from output
TUNNEL_ID=$(grep -oP 'Tunnel ID: \K[0-9a-f-]+' /tmp/tunnel-create.log)
if [ -z "$TUNNEL_ID" ]; then
    echo "❌ Failed to create tunnel"
    exit 1
fi

echo ""
echo "✅ Tunnel created: $TUNNEL_ID"

# Create config file
cat > ~/.cloudflared/config.yml << EOF
tunnel: $TUNNEL_ID
credentials-file: /home/rapheal/.cloudflared/${TUNNEL_ID}.json

ingress:
  - hostname: voice.yourdomain.com
    service: https://localhost:3443
    originRequest:
      noTLSVerify: true
      connectTimeout: 30s
      tcpKeepAlive: 30s
      keepAliveTimeout: 30s
  - service: http_status:404
EOF

echo "Config created at: ~/.cloudflared/config.yml"
echo ""
echo "📋 Next steps:"
echo "   1. Create a DNS record in Cloudflare Dashboard:"
echo "      - Type: CNAME"
echo "      - Name: voice (or your choice)"
echo "      - Target: ${TUNNEL_ID}.cfargotunnel.com"
echo "      - Proxy status: Proxied"
echo ""
echo "   2. Start your voice chat server (HTTPS):"
echo "      cargo run --release -- serve --https --cert cert.pem --key key.pem --port 3443 --host 0.0.0.0"
echo ""
echo "   3. In another terminal, start the tunnel:"
echo "      cloudflared tunnel run $TUNNEL_ID"
echo ""
echo "   4. Access from phone:"
echo "      https://voice.yourdomain.com"
echo ""
echo "   ✅ No certificate warnings! Real SSL from Cloudflare!"
echo ""
echo "📝 Save this tunnel ID: $TUNNEL_ID"

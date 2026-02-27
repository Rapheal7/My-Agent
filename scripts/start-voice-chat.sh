#!/bin/bash
# ~/start-voice-chat.sh
# Start the mobile voice chat HTTPS server

# Ensure cloudflared is in PATH
export PATH="$HOME/.local/bin:$PATH"

echo "=== 🎤 Mobile Voice Chat Server ==="
echo ""

# Check Tailscale
echo "1️⃣ Checking Tailscale..."
tailscale ip -4 > /dev/null 2>&1
if [ $? -eq 0 ]; then
    TAILSCALE_IP=$(tailscale ip -4)
    echo "   ✅ Tailscale IP: $TAILSCALE_IP"
else
    echo "   ❌ Tailscale not running. Start with: tailscale up"
    exit 1
fi

# Check certificates
echo ""
echo "2️⃣ Checking certificates..."
if [ ! -f "cert.pem" ] || [ ! -f "key.pem" ]; then
    echo "   📝 Generating self-signed certificates..."
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout key.pem -out cert.pem -days 365 \
        -subj "/C=US/ST=State/L=City/O=Organization/CN=localhost"
    echo "   ✅ Certificates created"
else
    echo "   ✅ Certificates exist"
fi

# Display connection info
echo ""
echo "3️⃣ Connection Instructions:"
echo "   📱 On your phone, open browser and visit:"
echo "   ┌──────────────────────────────────────┐"
echo "   │  https://$TAILSCALE_IP:3443/         │"
echo "   └──────────────────────────────────────┘"
echo ""
echo "   ⚠️  You will see a certificate warning (expected):"
echo "   • Chrome: Tap 'Advanced' → 'Proceed'"
echo "   • Safari: Tap 'Continue'"
echo ""
echo "   💡 Once you add your free Cloudflare domain, you can:"
echo "   • Set up Cloudflare Tunnel for NO certificate warnings"
echo "   • Use: https://voice.yourdomain.com"
echo ""
echo "   🎤 When the page loads:"
echo "   • Tap the microphone button 🎤 to START recording"
echo "   • Allow microphone access when prompted"
echo "   • Speak your message"
echo "   • Tap the button AGAIN to STOP and send"
echo ""

# Kill existing server
echo "4️⃣ Stopping any existing server..."
pkill -f "my_agent serve" 2>/dev/null
sleep 1

# Start server
echo ""
echo "5️⃣ Starting HTTPS server..."
echo "   Access URL: https://$TAILSCALE_IP:3443/"
echo ""

if [ -f "target/release/my_agent" ]; then
    target/release/my_agent serve \
        --https \
        --cert cert.pem \
        --key key.pem \
        --port 3443 \
        --host 0.0.0.0
else
    echo "   ❌ Server binary not found. Run: cargo build --release"
    exit 1
fi

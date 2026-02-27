# Mobile Voice Chat - Complete Setup & Usage Guide

## ✅ Current Server Status

| Component | Status | Details |
|-----------|--------|---------|
| **HTTPS Server** | ✅ Running | Port 3443 |
| **Tailscale Network** | ✅ Active | 100.125.204.83 |
| **Phone IP** | ✅ Connected | 100.89.8.82 |
| **Server URL** | ✅ Accessible | https://100.125.204.83:3443/ |

---

## 🔧 Problem 1: "Connection is Not Private" Warning

### Why This Happens
The warning appears because we're using a **self-signed certificate** (for development). Browsers show this warning because:
- The certificate wasn't issued by a trusted Certificate Authority (CA)
- This is a security precaution, not an actual danger
- For local Tailscale connections, this is **safe** - you control the connection

### Solutions (Choose One)

#### Option A: Use Cloudflare Tunnel (BEST - No Certificate Issues)
This is **recommended** for mobile access as it:
- Provides real SSL certificates (no warning)
- Works from anywhere (not just Tailscale)
- No port forwarding needed

```bash
# Install cloudflared
curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o cloudflared
chmod +x cloudflared
sudo mv cloudflared /usr/local/bin/

# Login to Cloudflare (one-time)
cloudflared tunnel login

# Create tunnel
cloudflared tunnel create my-agent

# Create config file
cat > ~/.cloudflared/config.yml << 'EOF'
tunnel: my-agent
credentials-file: /home/rapheal/.cloudflared/my-agent.json

ingress:
  - hostname: voice.yourdomain.com
    service: https://localhost:3443
    originRequest:
      noTLSVerify: true
  - service: http_status:404
EOF

# Start the tunnel (run in background)
cloudflared tunnel run my-agent
```

Then access: `https://voice.yourdomain.com` (no warning!)

---

#### Option B: Add Certificate to Phone Trust Store (Android Only)

**On your Android phone:**
1. Download the certificate from your server:
   ```bash
   # On your Linux system, get the certificate
   cat cert.pem
   ```
2. Copy the certificate content to your phone
3. Go to: **Settings → Security → Encryption & Credentials → Install a certificate → CA Certificate**
4. Paste the certificate content
5. Restart your browser

**Warning:** Only do this for development - don't add self-signed certs to production devices.

---

#### Option C: Use Local Domain Name (Easiest)

Create a local domain that bypasses certificate warnings:

```bash
# Create a script to start with local domain
cat > ~/start-voice-chat.sh << 'EOF'
#!/bin/bash
echo "=== Voice Chat Server ==="
echo ""
echo "To avoid certificate warnings, add this to your /etc/hosts file:"
echo "100.125.204.83  voice.local"
echo ""
echo "Then access: https://voice.local:3443/"
echo ""
echo "Starting server..."
target/release/my_agent serve --https --cert cert.pem --key key.pem --port 3443 --host 0.0.0.0
EOF

chmod +x ~/start-voice-chat.sh
```

**On your phone**, edit `/etc/hosts` (requires root) or use a DNS app like "DNS Changer" to map `voice.local` to `100.125.204.83`.

---

#### Option D: Accept the Warning (Quickest)

Just tap through the warning:
- **Chrome**: Tap "Advanced" → "Proceed to 100.125.204.83 (unsafe)"
- **Safari**: Tap "Continue" or "Visit Anyway"

This is safe for local Tailscale connections since you control both ends.

---

## 🎤 Problem 2: "Voice Error: Not-Allowed" / Microphone Issues

### Why This Happens
1. **Microphone permission denied** - Browser can't access your mic
2. **HTTPS requirement** - Most browsers require HTTPS for microphone access
3. **Browser compatibility** - Some browsers have limited Web Speech API support

### ✅ Fixed in New UI (Already Deployed!)

The improved UI now includes:

1. **Clear Permission Request** - Modal popup explaining microphone access
2. **Visual Feedback** - Shows when mic is listening/recording
3. **Better Error Messages** - Tells you exactly what's wrong
4. **Status Indicators** - Real-time feedback on voice activity

### How to Use Voice Chat on Android

1. **First-time setup:**
   - Visit `https://100.125.204.83:3443/`
   - Accept certificate warning (see above)
   - Tap the **microphone button** 🎤
   - **Allow** the microphone permission when prompted
   - You'll see "🎤 Listening... Speak now"

2. **To talk:**
   - Tap and **hold** the microphone button
   - Speak your message
   - Release the button when done
   - Wait for processing and response

3. **Visual indicators:**
   - 🔴 **Red pulsing** = Recording your voice
   - 🟢 **Green glow** = Listening (Web Speech API)
   - ⏳ **Status message** = Processing your audio
   - ✅ **Response** = AI replied

---

## 🚀 Complete Setup Script

Create this script for easy server startup:

```bash
cat > ~/start-voice-chat.sh << 'EOF'
#!/bin/bash
# ~/start-voice-chat.sh

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
echo "   ⚠️  You will see a certificate warning:"
echo "   • Chrome: Tap 'Advanced' → 'Proceed'"
echo "   • Safari: Tap 'Continue'"
echo ""
echo "   🎤 When the page loads:"
echo "   • Tap the microphone button 🎤"
echo "   • Allow microphone access"
echo "   • Hold to speak, release to send"
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

target/release/my_agent serve \
    --https \
    --cert cert.pem \
    --key key.pem \
    --port 3443 \
    --host 0.0.0.0

EOF

chmod +x ~/start-voice-chat.sh
echo "Script created: ~/start-voice-chat.sh"

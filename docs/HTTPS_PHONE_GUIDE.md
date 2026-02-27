# HTTPS Setup for Phone Access

This guide explains how to access your voice agent from your phone with HTTPS.

## Quick Start (Recommended: mkcert)

```bash
cd /home/rapheal/Projects/my-agent
./start-https-phone.sh
```

Then select option **1) mkcert**.

## Methods Compared

| Method | Ease | Phone Warning | Works Remotely | Best For |
|--------|------|---------------|----------------|----------|
| **mkcert** | Easy | ❌ No | ❌ Same WiFi only | Local testing |
| **Self-signed** | Easy | ⚠️ Yes | ❌ Same WiFi only | Quick setup |
| **Cloudflare Tunnel** | Medium | ❌ No | ✅ Yes | Remote access |

## Method 1: mkcert (RECOMMENDED)

Creates trusted certificates that won't show warnings on your phone.

### Prerequisites

```bash
# Ubuntu/Debian
sudo apt install mkcert libnss3-tools

# macOS
brew install mkcert
brew install nss  # For Firefox

# Initialize mkcert
mkcert -install
```

### Start Server

```bash
./start-https-phone.sh
# Select option 1
```

### Phone Setup

1. **Same WiFi**: Ensure phone and computer are on same WiFi
2. **Install CA**: Download `rootCA.pem` from computer to phone:
   ```bash
   # Find the CA certificate
   mkcert -CAROOT
   # Copy the rootCA.pem to your phone (email, cloud, etc.)
   ```
3. **Trust CA**:
   - **Android**: Settings → Security → Install from storage → Select rootCA.pem
   - **iOS**: Email the certificate → Tap to install → Settings → General → About → Certificate Trust Settings → Enable

4. **Open URL**: Visit `https://YOUR_COMPUTER_IP:8443/voice-client.html`

## Method 2: Self-Signed Certificates

Works immediately but phone will show security warnings.

### Start Server

```bash
./start-https-phone.sh
# Select option 2
```

### Phone Setup

1. **Open URL**: Visit `https://YOUR_COMPUTER_IP:8443/voice-client.html`

2. **Bypass Warning**:
   - **Chrome Android**: Tap "Advanced" → "Proceed to..."
   - **Safari iOS**: Tap "Show Details" → "visit this website"
   - **Samsung Internet**: Tap "Advanced" → "Proceed"

3. **Grant Permissions**: Allow microphone when prompted

## Method 3: Cloudflare Tunnel (Remote Access)

Access your agent from anywhere, not just home WiFi.

### Start Server

```bash
./start-https-phone.sh
# Select option 3
```

### Access

1. Wait for tunnel to start (10-20 seconds)
2. A URL like `https://my-agent-xxxx.trycloudflare.com` will be shown
3. Open this URL on your phone
4. **No certificate warnings!**

### Note

- URL changes each time you restart
- Connection goes through Cloudflare's network
- Slightly higher latency than local connection

## Method 4: Reverse Proxy (Advanced)

If you already have nginx or another reverse proxy set up.

### nginx Configuration

```nginx
server {
    listen 443 ssl http2;
    server_name voice.yourdomain.com;

    ssl_certificate /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:8080;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_cache_bypass $http_upgrade;

        # WebSocket timeout settings
        proxy_read_timeout 86400;
    }
}
```

### Start

```bash
./start-https-phone.sh
# Select option 4 (HTTP only)
```

## Troubleshooting

### "This site can't be reached"

1. Check computer's firewall:
   ```bash
   sudo ufw allow 8443/tcp  # Ubuntu
   sudo firewall-cmd --add-port=8443/tcp --permanent  # CentOS/RHEL
   ```

2. Verify IP address:
   ```bash
   hostname -I
   ```

3. Try accessing from computer first: `https://localhost:8443`

### "Microphone not working"

1. HTTPS is **required** for microphone access on mobile browsers
2. Check URL starts with `https://` not `http://`
3. Grant microphone permission in browser settings
4. Try refreshing the page

### "Certificate not trusted" (mkcert)

1. Make sure you installed the root CA on your phone
2. On iOS, also enable in Settings → General → About → Certificate Trust Settings
3. Try restarting your phone's browser

### LiveKit Connection Fails

1. Check `.env` file has correct credentials
2. Verify agent is running: `ps aux | grep livekit_agent`
3. Check agent logs: `tail -f voice-agent.log`

## Security Notes

- **mkcert**: Safe for local development, certificates are trusted only on your devices
- **Self-signed**: Safe for local use, but phone will warn about them
- **Cloudflare Tunnel**: Encrypted end-to-end, but traffic passes through Cloudflare
- **Never share** your LiveKit API keys or OpenRouter keys
- **Regenerate** Cloudflare Tunnel URL if you want to revoke access

## QR Code for Easy Access

Install qrencode to generate QR codes:

```bash
# Ubuntu/Debian
sudo apt install qrencode

# macOS
brew install qrencode
```

Then generate QR code:

```bash
qrencode -t ANSIUTF8 "https://YOUR_URL_HERE"
```

Or save as image:

```bash
qrencode -o qr-code.png "https://YOUR_URL_HERE"
```

## One-Line Phone Setup

After starting the server, scan this QR code with your phone:

```bash
# Get your IP and generate QR
IP=$(hostname -I | awk '{print $1}')
qrencode -t ANSIUTF8 "https://${IP}:8443/voice-client.html"
```

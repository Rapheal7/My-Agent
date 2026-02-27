# Mobile Voice Chat Setup with Tailscale + HTTPS

## Current Status

✅ **Tailscale Network**: Active
- Your Linux system: `100.125.204.83`
- Your Android phone: `100.89.8.82`
- Both devices are on the same Tailscale network

✅ **HTTPS Server**: Running
- Port: `3443`
- Protocol: HTTPS with TLS
- Certificate: Self-signed (for development)

## Access from Your Phone

### Option 1: Direct HTTPS Access (Recommended)

1. **On your phone**, open a browser and visit:
   ```
   https://100.125.204.83:3443/
   ```

2. **Accept the security warning** (self-signed certificate)
   - Chrome: Tap "Advanced" → "Proceed to 100.125.204.83 (unsafe)"
   - Safari: Tap "Continue" when prompted

3. **Access the voice chat interface**:
   - You'll see the AI agent web interface
   - Click the microphone button to start voice chat
   - The app will use the voice mode configured on your Linux system

### Option 2: Using Tailscale DNS (Easier)

If Tailscale DNS is working, you can use:
```
https://rapheal-system:3443/
```

Or if you have a custom hostname configured:
```
https://your-machine-name:3443/
```

## Voice Chat Configuration

### Check Current Voice Mode

The server automatically detects the best voice mode based on your configuration:

1. **LiveKit Agent AI** (if configured)
   - Full-duplex voice with back-channeling
   - Real-time speech-to-speech processing

2. **Moshi** (if configured)
   - Full-duplex with back-channeling
   - Low latency (~200ms)

3. **Hugging Face** (if HF API key set)
   - Full voice-to-voice processing
   - Speech-to-text + text-to-speech

4. **Local** (if Ollama is running)
   - Voice-to-text only
   - Text responses

5. **Sesame** (if local GGUF model path set)
   - Local GGUF model inference
   - Text responses

6. **Text Only** (default)
   - Text chat only
   - No voice processing

### Configure Voice Settings

Before starting the server, configure your preferred voice mode:

```bash
# Check current configuration
cargo run -- config --show

# Set up Moshi voice (Kaggle/Colab)
cargo run -- config --set-moshi-url wss://your-kaggle-url.ngrok.io/api/chat
cargo run -- config --set-moshi-voice NATF2

# Set up LiveKit (if you have LiveKit credentials)
cargo run -- config --set-livekit-url wss://your-project.livekit.cloud
cargo run -- config --set-livekit-key your-api-key
cargo run -- config --set-livekit-secret your-api-secret
cargo run -- config --set-livekit-voice-model openai/gpt-4o-realtime

# Set up local model path (for Sesame)
cargo run -- config --set-model-path /path/to/Sesame-13B-q8.gguf

# Set up voice chat model (OpenRouter)
cargo run -- config --set-voice-model x-ai/grok-4.1-fast
```

## Complete Setup Script

Create a startup script for easy mobile access:

```bash
#!/bin/bash
# ~/start-mobile-server.sh

echo "=== Starting Mobile Voice Chat Server ==="
echo ""

# Check Tailscale status
echo "Checking Tailscale..."
tailscale status | head -5
echo ""

# Get Tailscale IP
TAILSCALE_IP=$(tailscale ip -4)
echo "Your Tailscale IP: $TAILSCALE_IP"
echo ""

# Generate certificates if they don't exist
if [ ! -f "cert.pem" ] || [ ! -f "key.pem" ]; then
    echo "Generating self-signed certificates..."
    openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 365 \
        -subj "/C=US/ST=State/L=City/O=Organization/CN=localhost"
    echo ""
fi

# Start HTTPS server
echo "Starting HTTPS server on port 3443..."
echo "Access from your phone: https://$TAILSCALE_IP:3443/"
echo ""

cargo run -- serve --https --cert cert.pem --key key.pem --port 3443 --host 0.0.0.0
```

Make it executable:
```bash
chmod +x ~/start-mobile-server.sh
```

## Phone Setup Instructions

### On Android (Chrome)

1. **Open Chrome browser**
2. **Navigate to**: `https://100.125.204.83:3443/`
3. **Accept security warning** (self-signed cert)
4. **Grant microphone permission** when prompted
5. **Start voice chat** by clicking the microphone button

### On iOS (Safari)

1. **Open Safari browser**
2. **Navigate to**: `https://100.125.204.83:3443/`
3. **Accept security warning**
4. **Grant microphone permission**
5. **Start voice chat**

## Testing Voice Chat

### Test 1: Basic HTTP Request
```bash
# From your phone (via Tailscale)
curl -k https://100.125.204.83:3443/
```

### Test 2: WebSocket Connection
```bash
# Test WebSocket endpoint
curl -k -i -N -H "Connection: Upgrade" -H "Upgrade: websocket" \
  -H "Host: 100.125.204.83:3443" -H "Origin: https://100.125.204.83:3443" \
  https://100.125.204.83:3443/ws
```

## Production Considerations

### For Better Security (Recommended)

1. **Use Let's Encrypt certificates** instead of self-signed:
   ```bash
   # Install certbot
   sudo apt install certbot

   # Get certificate (requires domain name)
   sudo certbot certonly --standalone -d yourdomain.com

   # Use the certificates
   cargo run -- serve --https \
     --cert /etc/letsencrypt/live/yourdomain.com/fullchain.pem \
     --key /etc/letsencrypt/live/yourdomain.com/privkey.pem \
     --port 443
   ```

2. **Use Cloudflare Tunnel** (no port forwarding needed):
   ```bash
   # Install cloudflared
   curl -L https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64 -o cloudflared
   chmod +x cloudflared

   # Create tunnel
   cloudflared tunnel login
   cloudflared tunnel create my-agent

   # Run tunnel
   cloudflared tunnel run --url http://localhost:3000 my-agent
   ```

3. **Set up nginx reverse proxy**:
   ```nginx
   server {
       listen 443 ssl http2;
       server_name your-domain.com;

       ssl_certificate /path/to/cert.pem;
       ssl_certificate_key /path/to/key.pem;

       location / {
           proxy_pass http://127.0.0.1:3000;
           proxy_http_version 1.1;
           proxy_set_header Upgrade $http_upgrade;
           proxy_set_header Connection 'upgrade';
           proxy_set_header Host $host;
           proxy_set_header X-Real-IP $remote_addr;
           proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
           proxy_set_header X-Forwarded-Proto $scheme;
       }
   }
   ```

## Troubleshooting

### Can't connect from phone?
1. Check Tailscale is running: `tailscale status`
2. Check server is running: `ps aux | grep my_agent`
3. Check firewall: `sudo ufw status`
4. Try direct IP: `https://100.125.204.83:3443/`

### Voice not working?
1. Check voice mode is configured: `cargo run -- config --show`
2. Check microphone permission in browser
3. Check server log for errors: `tail -f /tmp/https_server.log`

### HTTPS certificate warnings?
- This is expected with self-signed certificates
- For production, use Let's Encrypt or Cloudflare

## Quick Reference Commands

```bash
# Start mobile HTTPS server
cargo run -- serve --https --cert cert.pem --key key.pem --port 3443 --host 0.0.0.0

# Check Tailscale IP
tailscale ip -4

# View server logs
tail -f /tmp/https_server.log

# Test from phone (replace with your actual IP)
curl -k https://100.125.204.83:3443/

# Stop all servers
pkill -f "my_agent serve"
```

## Next Steps

1. ✅ **Test HTTPS server** - Working!
2. ✅ **Verify Tailscale connectivity** - Working!
3. **Configure voice mode** - Set your preferred voice option
4. **Test voice chat on phone** - Access via browser
5. **Consider production setup** - Let's Encrypt or Cloudflare for security

# Voice Chat Setup for agent-assistant.org

This guide will help you set up voice chat on your HTTPS domain so you can talk with your agent and have it speak back to you.

## Quick Overview

Your agent now supports:
- **Voice-to-voice chat**: Speak to the agent, it responds with audio
- **Text-to-speech**: Type messages and have them spoken back
- **HTTPS/WSS**: Secure WebSocket connections for production

## Prerequisites

1. A server with a public IP address
2. Your domain `agent-assistant.org` pointing to your server
3. SSL certificates for HTTPS
4. A HuggingFace API key (for speech-to-text and text-to-speech)

## Step 1: Get SSL Certificates (Let's Encrypt)

Install certbot and obtain certificates:

```bash
# Install certbot
sudo apt update
sudo apt install certbot

# Obtain certificates (replace with your email)
sudo certbot certonly --standalone -d agent-assistant.org -d www.agent-assistant.org

# Certificates will be saved at:
# /etc/letsencrypt/live/agent-assistant.org/fullchain.pem
# /etc/letsencrypt/live/agent-assistant.org/privkey.pem
```

## Step 2: Get HuggingFace API Key

1. Go to https://huggingface.co/settings/tokens
2. Create a new access token
3. Copy the token for the next step

## Step 3: Configure the Agent

### Set the HuggingFace API key:

```bash
cd /home/rapheal/Projects/my-agent
cargo run -- config --set-hf-api-key YOUR_HF_API_KEY
```

### (Optional) Set a custom voice model:

```bash
# Use a different OpenRouter model for responses
cargo run -- config --set-voice-chat-model "x-ai/grok-4.1-fast"
```

Available models:
- `x-ai/grok-4.1-fast` (default, fast)
- `anthropic/claude-sonnet-4.5` (smart, slower)
- `google/gemini-flash-1.5` (balanced)
- `meta-llama/llama-3.1-70b` (good quality)

## Step 4: Build the Agent

```bash
cd /home/rapheal/Projects/my-agent
cargo build --release
```

The binary will be at `target/release/my-agent`.

## Step 5: Run the Server with HTTPS

### Option A: Direct HTTPS (Simplest)

```bash
sudo ./target/release/my-agent serve \
    --host 0.0.0.0 \
    --port 443 \
    --https \
    --cert /etc/letsencrypt/live/agent-assistant.org/fullchain.pem \
    --key /etc/letsencrypt/live/agent-assistant.org/privkey.pem
```

### Option B: Using systemd service

Create `/etc/systemd/system/my-agent.service`:

```ini
[Unit]
Description=My Agent Voice Chat Server
After=network.target

[Service]
Type=simple
User=www-data
WorkingDirectory=/home/rapheal/Projects/my-agent
ExecStart=/home/rapheal/Projects/my-agent/target/release/my-agent serve --host 0.0.0.0 --port 443 --https --cert /etc/letsencrypt/live/agent-assistant.org/fullchain.pem --key /etc/letsencrypt/live/agent-assistant.org/privkey.pem
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Enable and start:

```bash
sudo systemctl daemon-reload
sudo systemctl enable my-agent
sudo systemctl start my-agent
sudo systemctl status my-agent
```

### Option C: Behind nginx reverse proxy (Recommended for production)

Run agent on HTTP locally:

```bash
./target/release/my-agent serve --host 127.0.0.1 --port 3000
```

Configure nginx (`/etc/nginx/sites-available/agent-assistant.org`):

```nginx
server {
    listen 443 ssl http2;
    server_name agent-assistant.org www.agent-assistant.org;

    ssl_certificate /etc/letsencrypt/live/agent-assistant.org/fullchain.pem;
    ssl_certificate_key /etc/letsencrypt/live/agent-assistant.org/privkey.pem;

    # WebSocket support
    location /ws {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection "upgrade";
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_read_timeout 86400;
    }

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
    }
}

# Redirect HTTP to HTTPS
server {
    listen 80;
    server_name agent-assistant.org www.agent-assistant.org;
    return 301 https://$server_name$request_uri;
}
```

Enable the site:

```bash
sudo ln -s /etc/nginx/sites-available/agent-assistant.org /etc/nginx/sites-enabled/
sudo nginx -t
sudo systemctl reload nginx
```

## Step 6: Test the Voice Chat

1. Open `https://agent-assistant.org` in your browser
2. Allow microphone access when prompted
3. You should see "Connected" status
4. Click the microphone button (🎤) and speak
5. The agent will transcribe your speech, respond, and speak the response aloud

### Features:

- **🎤 Button (Red)**: Tap to start/stop voice recording
- **🔊 Speak Toggle**: Enable to have text responses spoken aloud
- **Real-time mode**: The agent can also work in real-time continuous mode

## Troubleshooting

### WebSocket connection fails

Check browser console (F12) for errors:

```
# Test WebSocket manually:
wss://agent-assistant.org/ws
```

Common issues:
- Firewall blocking port 443
- SSL certificate issues
- nginx not proxying WebSocket upgrade headers

### No audio playback

1. Check browser autoplay policies
2. Ensure audio isn't muted
3. Check debug console (press ` key) for errors

### Transcription fails

- Verify HuggingFace API key is set correctly
- Check HuggingFace API status: https://status.huggingface.co/
- HuggingFace free tier has rate limits

### TTS (Text-to-Speech) fails

- Same as above - requires HuggingFace API
- Try refreshing the page if models are "loading"

### Microphone not working

- Ensure you're on HTTPS (microphone requires secure context)
- Check browser permissions
- Try a different browser

## Updating SSL Certificates

Let's Encrypt certificates expire every 90 days. Set up auto-renewal:

```bash
# Test renewal
sudo certbot renew --dry-run

# Certbot usually sets up a cron job automatically
# If not, add to crontab:
sudo crontab -e
# Add line:
0 3 * * * /usr/bin/certbot renew --quiet --deploy-hook "systemctl reload nginx"
```

## Security Considerations

1. **Keep your HF API key secure** - it's stored in the system keyring
2. **Rate limiting** - the server has built-in rate limiting (100 req/min per IP)
3. **CSP headers** - content security policy headers are set for safety
4. **TLS 1.3 only** - modern encryption

## Advanced: Using LiveKit (Better Voice Quality)

For even better voice quality, consider setting up LiveKit:

1. Sign up at https://livekit.io/
2. Get your API credentials
3. Configure the agent:

```bash
cargo run -- config --set-livekit-url "wss://your-project.livekit.cloud"
cargo run -- config --set-livekit-key "your-api-key"
cargo run -- config --set-livekit-secret "your-api-secret"
```

LiveKit provides:
- Full-duplex conversation (interruptions work)
- Better audio quality
- Lower latency
- Noise cancellation

## Monitoring

View server logs:

```bash
# If running directly
RUST_LOG=info ./target/release/my-agent serve --https ...

# If using systemd
sudo journalctl -u my-agent -f
```

## Support

For issues or questions:
1. Check the debug console in the browser (press ` key)
2. Review server logs
3. File an issue on the project repository

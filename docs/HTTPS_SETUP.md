# HTTPS Setup for my-agent Web Server

The web server now supports HTTPS with TLS certificates. Here's how to set it up:

## Option 1: Direct HTTPS with Self-Signed Certificate (Development)

### Generate self-signed certificate:
```bash
openssl req -x509 -newkey rsa:2048 -nodes -keyout key.pem -out cert.pem -days 365 \
  -subj "/C=US/ST=State/L=City/O=Organization/CN=localhost"
```

### Run HTTPS server:
```bash
cargo run -- serve --https --cert cert.pem --key key.pem --port 3443
```

### Access the server:
- **HTTPS URL**: https://localhost:3443
- **Note**: Browser will show security warning for self-signed certificate
- Use `-k` flag with curl: `curl -k https://localhost:3443/`

## Option 2: Reverse Proxy (Production Recommended)

### Using nginx:

```nginx
server {
    listen 443 ssl http2;
    server_name your-domain.com;

    # SSL certificates
    ssl_certificate /path/to/fullchain.pem;
    ssl_certificate_key /path/to/privkey.pem;

    location / {
        proxy_pass http://127.0.0.1:3000;
        proxy_http_version 1.1;
        proxy_set_header Upgrade $http_upgrade;
        proxy_set_header Connection 'upgrade';
        proxy_set_header Host $host;
        proxy_set_header X-Real-IP $remote_addr;
        proxy_set_header X-Forwarded-For $proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto $scheme;
        proxy_cache_bypass $http_upgrade;
    }
}
```

### Run the server on HTTP:
```bash
cargo run -- serve --port 3000
```

### Access via HTTPS:
- https://your-domain.com

## Option 3: Cloudflare Tunnel

1. Install cloudflared: https://developers.cloudflare.com/cloudflare-one/connections/connect-apps/
2. Create a tunnel:
   ```bash
   cloudflared tunnel login
   cloudflared tunnel create my-agent
   ```
3. Route traffic:
   ```bash
   cloudflared tunnel route dns my-agent your-subdomain
   ```
4. Run tunnel:
   ```bash
   cloudflared tunnel run --url http://localhost:3000 my-agent
   ```

## Security Notes

- **Self-signed certificates** are for development only
- **Production**: Use Let's Encrypt (certbot) or a trusted CA
- **Store private keys securely**: Use keyring or environment variables
- **WebSocket connections**: Ensure proxy passes WebSocket upgrade headers

## Testing

### Test HTTPS server:
```bash
# With self-signed cert (skip verification)
curl -k https://localhost:3443/

# With verbose output
curl -k -v https://localhost:3443/
```

### Test HTTP server (for proxy):
```bash
curl http://localhost:3000/
```

## Command Line Options

```bash
my-agent serve --help

Options:
  --port <PORT>        Port to listen on [default: 3000]
  --host <HOST>        Host to bind to [default: 0.0.0.0]
  --https              Enable HTTPS mode
  --cert <CERT>        Path to TLS certificate (PEM format)
  --key <KEY>          Path to TLS private key (PEM format)
```

## Examples

```bash
# HTTP server (default)
my-agent serve --port 3000

# HTTPS with self-signed cert (development)
my-agent serve --https --cert cert.pem --key key.pem --port 3443

# HTTPS with production certificates
my-agent serve --https --cert /etc/ssl/fullchain.pem --key /etc/ssl/privkey.pem --port 443

# HTTP server behind reverse proxy
my-agent serve --port 3000 --host 127.0.0.1
```

## Troubleshooting

### Certificate errors:
- Ensure cert and key files exist and are readable
- Check file permissions (key file should be 600)
- Verify certificate chain is complete

### Port conflicts:
- Check if port is already in use: `netstat -tlnp | grep <port>`
- Use a different port if needed

### WebSocket not working through proxy:
- Ensure proxy headers are set correctly
- Check that upgrade headers are passed through
- Verify WebSocket connection in browser console

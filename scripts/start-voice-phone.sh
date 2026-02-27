#!/bin/bash
# Voice Agent Phone Access - Multiple HTTPS Options
# Supports: Tailscale HTTPS (recommended), Cloudflare Tunnel, or Self-signed

set -e

cd "$(dirname "$0")"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
CYAN='\033[0;36m'
NC='\033[0m'

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Stopping voice agent...${NC}"
    kill $HTTP_PID 2>/dev/null || true
    kill $AGENT_PID 2>/dev/null || true
    kill $TUNNEL_PID 2>/dev/null || true
    exit 0
}
trap cleanup INT TERM

echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Voice Agent - Phone Access Setup${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""

# Check environment
if [ ! -f ".env" ]; then
    echo -e "${RED}Error: .env file not found${NC}"
    exit 1
fi
export $(grep -v '^#' .env | xargs)

# Check Tailscale status
echo -e "${BLUE}Checking network options...${NC}"
TAILSCALE_IP=$(tailscale ip -4 2>/dev/null || echo "")
TAILSCALE_HOST=$(tailscale status --self 2>/dev/null | awk '{print $2}' || echo "")
LOCAL_IP=$(hostname -I 2>/dev/null | awk '{print $1}' || echo "")

if [ -n "$TAILSCALE_IP" ]; then
    echo -e "${GREEN}✓ Tailscale active${NC}"
    echo "  Tailscale IP: $TAILSCALE_IP"
    echo "  Tailscale hostname: $TAILSCALE_HOST"
fi

# Menu
echo ""
echo -e "${CYAN}Choose your phone access method:${NC}"
echo ""
echo "1) Tailscale HTTPS (RECOMMENDED)"
echo "   - Uses Tailscale's built-in HTTPS certificates"
echo "   - No browser warnings, works anywhere Tailscale works"
echo "   - URL: https://$TAILSCALE_HOST"
echo ""
echo "2) Cloudflare Tunnel"
echo "   - Public URL, works from anywhere"
echo "   - No browser warnings"
echo "   - Domain: voice.my-agent.com (if DNS configured)"
echo ""
echo "3) Local HTTPS (Self-signed)"
echo "   - Works on same WiFi only"
echo "   - Browser will show security warning"
echo "   - URL: https://$LOCAL_IP:8443"
echo ""
echo "4) HTTP Only (no HTTPS)"
echo "   - For testing only"
echo "   - URL: http://$LOCAL_IP:8080"
echo ""

read -p "Enter choice (1-4): " choice

case $choice in
    1)
        # Tailscale HTTPS
        if [ -z "$TAILSCALE_HOST" ]; then
            echo -e "${RED}Error: Tailscale not running${NC}"
            exit 1
        fi

        echo -e "${BLUE}Starting with Tailscale HTTPS...${NC}"

        # Check if tailscale serve is available
        if ! tailscale serve --help &>/dev/null; then
            echo -e "${YELLOW}Note: Using Tailscale Funnel (requires HTTPS)${NC}"
        fi

        # Start the HTTP server on port 8080 (Tailscale will proxy HTTPS)
        python3 simple_server.py 8080 &
        HTTP_PID=$!

        # Start tailscale serve for HTTPS
        echo -e "${BLUE}Enabling Tailscale HTTPS...${NC}"
        tailscale serve --https=443 --set-path=/ http://localhost:8080 &
        TUNNEL_PID=$!

        URL="https://$TAILSCALE_HOST/voice-client.html"
        ;;

    2)
        # Cloudflare Tunnel
        echo -e "${BLUE}Starting with Cloudflare Tunnel...${NC}"

        if [ ! -f "$HOME/.cloudflared/config.yml" ]; then
            echo -e "${RED}Error: Cloudflare Tunnel not configured${NC}"
            exit 1
        fi

        # Start HTTP server
        python3 simple_server.py 8080 &
        HTTP_PID=$!
        sleep 2

        # Start cloudflared
        $HOME/.local/bin/cloudflared tunnel run 4c088f83-1e2c-4e37-b7bc-0499837d8196 &
        TUNNEL_PID=$!
        sleep 3

        URL="https://voice.my-agent.com/voice-client.html"
        ;;

    3)
        # Local HTTPS
        echo -e "${BLUE}Starting with Local HTTPS...${NC}"

        # Create certs directory if needed
        mkdir -p certs

        # Generate self-signed certificate
        if [ ! -f "certs/cert.pem" ]; then
            echo -e "${YELLOW}Generating self-signed certificate...${NC}"
            openssl req -x509 -newkey rsa:2048 -nodes \
                -keyout certs/key.pem -out certs/cert.pem -days 365 \
                -subj "/C=US/ST=State/L=City/O=MyAgent/CN=$LOCAL_IP" 2>/dev/null
            chmod 600 certs/key.pem
        fi

        PORT=8443
        python3 simple_server.py $PORT certs/cert.pem certs/key.pem &
        HTTP_PID=$!

        URL="https://$LOCAL_IP:$PORT/voice-client.html"
        ;;

    4)
        # HTTP Only
        echo -e "${BLUE}Starting with HTTP only...${NC}"

        python3 simple_server.py 8080 &
        HTTP_PID=$!

        URL="http://$LOCAL_IP:8080/voice-client.html"
        ;;

    *)
        echo -e "${RED}Invalid choice${NC}"
        exit 1
        ;;
esac

# Start the agent
echo -e "${BLUE}Starting LiveKit Agent...${NC}"
if [ -f "livekit_agent_v2.py" ]; then
    python3 livekit_agent_v2.py --room voice-session &
else
    python3 livekit_agent.py --room voice-session &
fi
AGENT_PID=$!
sleep 2

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Voice Agent Ready!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "${BLUE}📱 OPEN THIS URL ON YOUR PHONE:${NC}"
echo ""
echo -e "   ${YELLOW}$URL${NC}"
echo ""

# Show QR code if available
if command -v qrencode &> /dev/null; then
    echo -e "${BLUE}📱 Or scan this QR code:${NC}"
    echo ""
    qrencode -t ANSIUTF8 "$URL"
    echo ""
fi

echo -e "${YELLOW}Press Ctrl+C to stop${NC}"
echo ""

# Keep running
wait

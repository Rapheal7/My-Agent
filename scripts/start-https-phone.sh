#!/bin/bash
# Start Voice Agent with HTTPS for Phone Access
# Supports multiple methods: mkcert (recommended), self-signed, or Cloudflare Tunnel

set -e

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

cd "$(dirname "$0")"

cleanup() {
    echo -e "\n${YELLOW}Shutting down...${NC}"
    if [ -n "$HTTP_PID" ]; then kill $HTTP_PID 2>/dev/null || true; fi
    if [ -n "$AGENT_PID" ]; then kill $AGENT_PID 2>/dev/null || true; fi
    if [ -n "$TUNNEL_PID" ]; then kill $TUNNEL_PID 2>/dev/null || true; fi
    exit 0
}

trap cleanup INT TERM

echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Voice Agent - HTTPS Phone Setup${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""

# Check for .env
if [ ! -f ".env" ]; then
    echo -e "${RED}Error: .env file not found!${NC}"
    exit 1
fi

export $(grep -v '^#' .env | xargs)

if [ -z "$OPENROUTER_API_KEY" ] || [ "$OPENROUTER_API_KEY" = "your_openrouter_key_here" ]; then
    echo -e "${RED}Error: OPENROUTER_API_KEY not set in .env${NC}"
    echo "Get free key: https://openrouter.ai/keys"
    exit 1
fi

# Function to get IP
get_ip() {
    hostname -I 2>/dev/null | awk '{print $1}' || echo ""
}

# Menu for HTTPS method
echo -e "${BLUE}Choose HTTPS method:${NC}"
echo ""
echo "1) mkcert - Trusted local certificates (RECOMMENDED)"
echo "   - Phone will NOT show security warnings"
echo "   - Works on same WiFi network"
echo ""
echo "2) Self-signed certificates"
echo "   - Phone will show 'Not Secure' warning (accept to continue)"
echo "   - Works on same WiFi network"
echo ""
echo "3) Cloudflare Tunnel - Remote access anywhere"
echo "   - Works from anywhere (not just same WiFi)"
echo "   - Public URL like: https://my-agent-xxxx.trycloudflare.com"
echo "   - No certificate warnings"
echo ""
echo "4) Start HTTP only (for reverse proxy users)"
echo ""

read -p "Select option (1-4): " choice

case $choice in
    1)
        echo -e "${BLUE}Setting up mkcert...${NC}"

        # Check if mkcert is installed
        if ! command -v mkcert &> /dev/null; then
            echo -e "${YELLOW}mkcert not found. Installing...${NC}"

            # Try to install mkcert
            if command -v apt &> /dev/null; then
                sudo apt update && sudo apt install -y mkcert libnss3-tools
            elif command -v brew &> /dev/null; then
                brew install mkcert
                brew install nss  # For Firefox
            else
                echo -e "${RED}Please install mkcert manually:${NC}"
                echo "  https://github.com/FiloSottile/mkcert#installation"
                exit 1
            fi

            # Initialize mkcert
            mkcert -install
        fi

        # Get local IP
        LOCAL_IP=$(get_ip)
        if [ -z "$LOCAL_IP" ]; then
            echo -e "${RED}Could not detect local IP address${NC}"
            exit 1
        fi

        # Generate certificates
        echo -e "${BLUE}Generating trusted certificate for localhost and $LOCAL_IP...${NC}"
        mkdir -p certs
        mkcert -cert-file certs/cert.pem -key-file certs/key.pem localhost 127.0.0.1 ::1 $LOCAL_IP 2>/dev/null || {
            echo -e "${YELLOW}Trying with just localhost...${NC}"
            mkcert -cert-file certs/cert.pem -key-file certs/key.pem localhost 127.0.0.1
        }

        CERT_FILE="certs/cert.pem"
        KEY_FILE="certs/key.pem"
        PORT=8443
        PROTOCOL="https"

        echo -e "${GREEN}✓ Trusted certificates generated${NC}"
        ;;

    2)
        echo -e "${BLUE}Using self-signed certificates...${NC}"

        # Check if certs exist
        if [ ! -f "key.pem" ] || [ ! -f "cert.pem" ]; then
            echo -e "${YELLOW}Creating self-signed certificates...${NC}"
            openssl req -x509 -newkey rsa:2048 -nodes \
                -keyout key.pem -out cert.pem -days 365 \
                -subj "/C=US/ST=State/L=City/O=MyAgent/CN=localhost" \
                2>/dev/null || {
                echo -e "${RED}Failed to create certificates. Make sure openssl is installed.${NC}"
                exit 1
            }
        fi

        CERT_FILE="cert.pem"
        KEY_FILE="key.pem"
        PORT=8443
        PROTOCOL="https"

        echo -e "${GREEN}✓ Self-signed certificates ready${NC}"
        echo -e "${YELLOW}Note: Phone will show 'Not Secure' warning - tap 'Advanced' → 'Proceed'${NC}"
        ;;

    3)
        echo -e "${BLUE}Setting up Cloudflare Tunnel...${NC}"

        # Check if cloudflared is installed
        if ! command -v cloudflared &> /dev/null; then
            echo -e "${YELLOW}Installing cloudflared...${NC}"

            # Install cloudflared
            if command -v apt &> /dev/null; then
                curl -L --output /tmp/cloudflared.deb https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64.deb
                sudo dpkg -i /tmp/cloudflared.deb
            else
                # Try other methods
                curl -L --output cloudflared https://github.com/cloudflare/cloudflared/releases/latest/download/cloudflared-linux-amd64
                chmod +x cloudflared
                sudo mv cloudflared /usr/local/bin/
            fi
        fi

        # We'll start HTTP server and tunnel it
        PORT=8080
        PROTOCOL="https"
        USE_TUNNEL=true
        ;;

    4)
        echo -e "${BLUE}Starting HTTP server...${NC}"
        PORT=8080
        PROTOCOL="http"
        CERT_FILE=""
        KEY_FILE=""
        ;;

    *)
        echo -e "${RED}Invalid option${NC}"
        exit 1
        ;;
esac

echo ""

# Start HTTP/HTTPS server
echo -e "${BLUE}Starting web server...${NC}"
if [ "$PROTOCOL" = "https" ] && [ -n "$CERT_FILE" ]; then
    python3 simple_server.py $PORT "$CERT_FILE" "$KEY_FILE" &
else
    python3 simple_server.py $PORT &
fi
HTTP_PID=$!
sleep 2

# Start Cloudflare Tunnel if selected
if [ "$USE_TUNNEL" = true ]; then
    echo -e "${BLUE}Starting Cloudflare Tunnel...${NC}"
    cloudflared tunnel --url http://localhost:$PORT &
    TUNNEL_PID=$!

    # Wait for tunnel to establish
    sleep 5

    # Get tunnel URL
    TUNNEL_URL=""
    for i in {1..10}; do
        TUNNEL_URL=$(curl -s http://localhost:4040/api/tunnels 2>/dev/null | grep -o 'https://[^"]*\.trycloudflare\.com' | head -1)
        if [ -n "$TUNNEL_URL" ]; then
            break
        fi
        sleep 2
    done

    if [ -n "$TUNNEL_URL" ]; then
        PUBLIC_URL="$TUNNEL_URL/voice-client.html"
    else
        echo -e "${YELLOW}Tunnel starting... check output above for URL${NC}"
        PUBLIC_URL="Check output above for https://xxxx.trycloudflare.com URL"
    fi
fi

# Start the agent
echo -e "${BLUE}Starting LiveKit Agent...${NC}"
if [ -f "livekit_agent_v2.py" ]; then
    python3 livekit_agent_v2.py --room voice-session &
else
    python3 livekit_agent.py --room voice-session &
fi
AGENT_PID=$!

# Get IP for display
LOCAL_IP=$(get_ip)

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Voice Agent Running!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""

if [ "$USE_TUNNEL" = true ]; then
    echo -e "${YELLOW}🌐 PUBLIC URL (works from anywhere):${NC}"
    echo "   $PUBLIC_URL"
    echo ""
    echo -e "${BLUE}📱 Scan this QR code with your phone:${NC}"
    echo ""
    if command -v qrencode &> /dev/null && [ -n "$TUNNEL_URL" ]; then
        qrencode -t ANSIUTF8 "$PUBLIC_URL"
    else
        echo "   (Install qrencode for QR code: sudo apt install qrencode)"
    fi
    echo ""
fi

echo -e "${BLUE}📱 PHONE ACCESS (same WiFi):${NC}"
if [ -n "$LOCAL_IP" ]; then
    echo "   $PROTOCOL://$LOCAL_IP:$PORT/voice-client.html"
    echo ""
fi

echo -e "${BLUE}💻 LOCAL ACCESS:${NC}"
echo "   $PROTOCOL://localhost:$PORT/voice-client.html"
echo ""

if [ "$PROTOCOL" = "https" ] && [ "$CERT_FILE" = "cert.pem" ]; then
    echo -e "${YELLOW}⚠️  Self-signed certificate instructions:${NC}"
    echo "   1. Open the URL on your phone"
    echo "   2. You'll see 'Not Secure' or 'Your connection is not private'"
    echo "   3. Tap 'Advanced' → 'Proceed' (Android) or 'Details' → 'visit this website' (iOS)"
    echo ""
fi

echo -e "${YELLOW}Press Ctrl+C to stop${NC}"
echo ""

wait

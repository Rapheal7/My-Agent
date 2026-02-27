#!/bin/bash
# Quick HTTPS server for phone access using existing certificates

set -e

cd "$(dirname "$0")"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

# Cleanup function
cleanup() {
    echo -e "\n${YELLOW}Stopping...${NC}"
    kill $HTTP_PID 2>/dev/null || true
    kill $AGENT_PID 2>/dev/null || true
    exit 0
}
trap cleanup INT TERM

echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Voice Agent - Phone HTTPS Setup${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""

# Check certificates
if [ ! -f "cert.pem" ] || [ ! -f "key.pem" ]; then
    echo -e "${YELLOW}Creating self-signed certificates...${NC}"
    openssl req -x509 -newkey rsa:2048 -nodes \
        -keyout key.pem -out cert.pem -days 365 \
        -subj "/C=US/ST=State/L=City/O=MyAgent/CN=localhost" 2>/dev/null
    chmod 600 key.pem
    echo -e "${GREEN}✓ Certificates created${NC}"
fi

# Get local IP
LOCAL_IP=$(hostname -I 2>/dev/null | awk '{print $1}' || echo "")

if [ -z "$LOCAL_IP" ]; then
    echo -e "${RED}Could not detect local IP address${NC}"
    echo "Make sure you're connected to WiFi"
    exit 1
fi

PORT=8443

# Load environment
export $(grep -v '^#' .env | xargs)

# Check dependencies
if [ -z "$OPENROUTER_API_KEY" ] || [ "$OPENROUTER_API_KEY" = "your_openrouter_key_here" ]; then
    echo -e "${RED}Error: OPENROUTER_API_KEY not set in .env${NC}"
    echo "Get free key: https://openrouter.ai/keys"
    exit 1
fi

# Start HTTPS server
echo -e "${BLUE}Starting HTTPS server on port $PORT...${NC}"
python3 simple_server.py $PORT cert.pem key.pem &
HTTP_PID=$!
sleep 2

# Start agent
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
echo -e "${GREEN}  System Ready!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo -e "${BLUE}📱 PHONE URL (open this on your phone):${NC}"
echo ""
echo -e "   ${YELLOW}https://$LOCAL_IP:$PORT/voice-client.html${NC}"
echo ""
echo -e "${BLUE}💻 COMPUTER URL:${NC}"
echo "   https://localhost:$PORT/voice-client.html"
echo ""
echo -e "${YELLOW}⚠️  IMPORTANT - Phone Setup:${NC}"
echo ""
echo "1. Connect phone to same WiFi as this computer"
echo "2. Open the PHONE URL above in Chrome or Safari"
echo "3. You'll see a security warning - tap:"
echo "   • Android Chrome: 'Advanced' → 'Proceed to...'"
echo "   • iOS Safari: 'Show Details' → 'visit this website'"
echo "4. Tap the microphone button and allow permissions"
echo ""

# Generate QR code if qrencode is available
if command -v qrencode &> /dev/null; then
    echo -e "${BLUE}📱 QR Code (scan with phone camera):${NC}"
    echo ""
    qrencode -t ANSIUTF8 "https://$LOCAL_IP:$PORT/voice-client.html"
    echo ""
fi

echo -e "${YELLOW}Press Ctrl+C to stop${NC}"
echo ""

# Keep running
wait

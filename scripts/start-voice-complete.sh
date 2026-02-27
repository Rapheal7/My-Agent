#!/bin/bash
# Complete Voice Agent Startup Script
# This starts both the HTTP server and the LiveKit agent

set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

cd "$(dirname "$0")"

# Function to cleanup on exit
cleanup() {
    echo -e "\n${YELLOW}Shutting down...${NC}"
    if [ -n "$HTTP_PID" ]; then
        kill $HTTP_PID 2>/dev/null || true
    fi
    if [ -n "$AGENT_PID" ]; then
        kill $AGENT_PID 2>/dev/null || true
    fi
    exit 0
}

trap cleanup INT TERM

echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  My Agent - Voice Assistant Setup${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""

# Check for .env file
if [ ! -f ".env" ]; then
    echo -e "${RED}Error: .env file not found!${NC}"
    echo "Creating .env template..."
    cat > .env << 'EOF'
LIVEKIT_API_KEY=APIG3jFfastPMAW
LIVEKIT_API_SECRET=7hsvSaqzQPpCmkt1Wj4vRACZljbf31qt3oJ4oc3n4WB
LIVEKIT_URL=wss://my-agent-t6shkefq.livekit.cloud
OPENROUTER_API_KEY=your_openrouter_key_here
EOF
    echo -e "${YELLOW}Please edit .env and add your OPENROUTER_API_KEY${NC}"
    echo "Get your free key at: https://openrouter.ai/keys"
    exit 1
fi

# Load environment variables
export $(grep -v '^#' .env | xargs)

# Check for required variables
if [ -z "$OPENROUTER_API_KEY" ] || [ "$OPENROUTER_API_KEY" = "your_openrouter_key_here" ]; then
    echo -e "${RED}Error: OPENROUTER_API_KEY not set in .env${NC}"
    echo "Get your free key at: https://openrouter.ai/keys"
    exit 1
fi

# Ensure static directory exists and copy HTML file
mkdir -p static
if [ -f "static/voice-client.html" ]; then
    echo -e "${GREEN}✓ Voice client HTML found${NC}"
else
    echo -e "${YELLOW}! Creating voice client HTML...${NC}"
    # The file should already exist from previous step
fi

# Check if pip dependencies are installed
echo ""
echo -e "${BLUE}Checking dependencies...${NC}"
python3 -c "import livekit, livekit.agents, faster_whisper, piper" 2>/dev/null || {
    echo -e "${YELLOW}Installing dependencies...${NC}"
    pip install livekit livekit-agents faster-whisper piper-tts
}

# Download Piper voice if needed
PIPER_VOICE_DIR="$HOME/.local/share/piper"
PIPER_VOICE_FILE="$PIPER_VOICE_DIR/en_US-lessac-medium.onnx"
if [ ! -f "$PIPER_VOICE_FILE" ]; then
    echo -e "${BLUE}Downloading Piper voice...${NC}"
    mkdir -p "$PIPER_VOICE_DIR"
    wget -q --show-progress -O "$PIPER_VOICE_FILE" \
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx" || {
        echo -e "${YELLOW}Warning: Could not download Piper voice${NC}"
    }
    wget -q --show-progress -O "${PIPER_VOICE_FILE}.json" \
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json" || true
fi

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Configuration Summary${NC}"
echo -e "${GREEN}========================================${NC}"
echo "LiveKit URL: $LIVEKIT_URL"
echo "Room: voice-session"
echo "STT: Faster-Whisper (local, free)"
echo "LLM: Grok 4.1 Fast (OpenRouter, free tier)"
echo "TTS: Piper (local, free)"
echo ""

# Start HTTP server
echo -e "${BLUE}Starting HTTP server...${NC}"
python3 simple_server.py 8080 &
HTTP_PID=$!
echo -e "${GREEN}✓ HTTP server started (PID: $HTTP_PID)${NC}"

# Wait a moment for server to start
sleep 1

# Get IP addresses for access info
echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  Access Information${NC}"
echo -e "${GREEN}========================================${NC}"
echo "Local:    http://localhost:8080/voice-client.html"

# Try to get network IP
IP_ADDR=$(hostname -I 2>/dev/null | awk '{print $1}' || echo "")
if [ -n "$IP_ADDR" ]; then
    echo "Network:  http://$IP_ADDR:8080/voice-client.html"
fi
echo ""

# Start the LiveKit agent
echo -e "${BLUE}Starting LiveKit Agent...${NC}"
echo -e "${YELLOW}The agent will connect when you open the browser client${NC}"
echo ""

# Use the improved agent if available
if [ -f "livekit_agent_v2.py" ]; then
    python3 livekit_agent_v2.py --room voice-session &
else
    python3 livekit_agent.py --room voice-session &
fi
AGENT_PID=$!
echo -e "${GREEN}✓ Agent started (PID: $AGENT_PID)${NC}"

echo ""
echo -e "${GREEN}========================================${NC}"
echo -e "${GREEN}  System Running!${NC}"
echo -e "${GREEN}========================================${NC}"
echo ""
echo "1. Open the voice client in your browser:"
echo "   http://localhost:8080/voice-client.html"
echo ""
echo "2. Tap the microphone button to connect"
echo ""
echo "3. The agent will join automatically"
echo ""
echo -e "${YELLOW}Press Ctrl+C to stop all services${NC}"
echo ""

# Wait for both processes
wait

#!/bin/bash
# Start both the Rust HTTPS server and LiveKit Agent
#
# This creates a complete voice chat system with FREE models:
# - Rust HTTPS server: Handles web UI and WebSocket connections
# - LiveKit Agent: Handles real-time voice processing
#   - STT: Whisper (local, free)
#   - LLM: Grok 4.1 Fast (OpenRouter - free tier)
#   - TTS: Piper (local, free)

set -e

# Colors
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
RED='\033[0;31m'
NC='\033[0m'

echo -e "${GREEN}=== My Agent Voice Chat System ===${NC}"
echo -e "${BLUE}Using FREE Open-Source Models:${NC}"
echo "  STT: Whisper (local)"
echo "  LLM: Grok 4.1 Fast (OpenRouter - free tier)"
echo "  TTS: Piper (local)"
echo ""

# LiveKit configuration (from config.toml)
export LIVEKIT_URL="wss://my-agent-t6shkefq.livekit.cloud"
export LIVEKIT_API_KEY="APIG3jFfastPMAW"
export LIVEKIT_API_SECRET="7hsvSaqzQPpCmkt1Wj4vRACZljbf31qt3oJ4oc3n4WB"

# Check required API key
if [ -z "$OPENROUTER_API_KEY" ]; then
    echo -e "${RED}Missing OPENROUTER_API_KEY!${NC}"
    echo "Get your free key at: https://openrouter.ai/keys"
    echo "export OPENROUTER_API_KEY=your_key"
    exit 1
fi

# Change to project directory
cd "$(dirname "$0")"

# Kill any existing processes
pkill -f "my_agent serve" 2>/dev/null || true
sleep 1

# Start the Rust HTTPS server in background
echo -e "${BLUE}Starting Rust HTTPS server...${NC}"
cargo run -- serve --https --cert cert.pem --key key.pem &
RUST_PID=$!
echo "Rust server PID: $RUST_PID"

# Wait for Rust server to start
sleep 3

# Check if Rust server is running
if ! kill -0 $RUST_PID 2>/dev/null; then
    echo -e "${RED}Error: Rust server failed to start${NC}"
    exit 1
fi

echo -e "${GREEN}Rust HTTPS server running at https://localhost:3000${NC}"

# Download Piper voice if needed
PIPER_VOICE_DIR="$HOME/.local/share/piper"
PIPER_VOICE_FILE="$PIPER_VOICE_DIR/en_US-lessac-medium.onnx"
if [ ! -f "$PIPER_VOICE_FILE" ]; then
    echo -e "${YELLOW}Downloading Piper voice...${NC}"
    mkdir -p "$PIPER_VOICE_DIR"
    wget -q -O "$PIPER_VOICE_FILE" "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx"
    wget -q -O "${PIPER_VOICE_FILE}.json" "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json"
fi

# Start LiveKit Agent (explicit room mode for direct connection)
echo -e "${BLUE}Starting LiveKit Agent...${NC}"
python3 livekit_agent.py --room voice-session > voice-agent.log 2>&1 &
AGENT_PID=$!
echo $AGENT_PID > .voice-agent.pid
echo "LiveKit Agent PID: $AGENT_PID"

# Handle shutdown
cleanup() {
    echo ""
    echo -e "${YELLOW}Shutting down...${NC}"
    kill $RUST_PID 2>/dev/null || true
    [ -n "$AGENT_PID" ] && kill $AGENT_PID 2>/dev/null || true
    echo "Goodbye!"
    exit 0
}

trap cleanup SIGINT SIGTERM

echo ""
echo -e "${GREEN}=== System Ready ===${NC}"
echo "Access from your phone at: https://<YOUR_IP>:3000"
echo "Voice mode: LiveKit Agent with Whisper + Grok 4.1 Fast + Piper"
echo ""
echo "Press Ctrl+C to stop"
echo ""

# Wait for processes
wait
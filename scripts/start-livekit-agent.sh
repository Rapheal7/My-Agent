#!/bin/bash
# Start LiveKit Voice Agent with Local Open-Source Models
#
# This uses FREE, LOCAL models:
# - STT: Whisper (runs locally on your machine)
# - LLM: Grok 4.1 Fast via OpenRouter (free, smart)
# - TTS: Piper (runs locally on your machine)
#
# Required environment variables:
# - OPENROUTER_API_KEY: Your OpenRouter API key (for Grok 4.1 Fast)
# - LIVEKIT_URL: Your LiveKit server URL (e.g., wss://your-project.livekit.cloud)
# - LIVEKIT_API_KEY: Your LiveKit API key
# - LIVEKIT_API_SECRET: Your LiveKit API secret

set -e

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

echo -e "${GREEN}=== LiveKit Voice Agent ===${NC}"
echo -e "${BLUE}Using FREE Open-Source Models:${NC}"
echo "  STT: Whisper (local)"
echo "  LLM: Grok 4.1 Fast (OpenRouter - free tier)"
echo "  TTS: Piper (local)"
echo ""

# Check for required environment variables
if [ -z "$OPENROUTER_API_KEY" ]; then
    echo -e "${RED}Missing OPENROUTER_API_KEY!${NC}"
    echo ""
    echo "Get your free key at: https://openrouter.ai/keys"
    echo "Export it like this:"
    echo "  export OPENROUTER_API_KEY=your_key_here"
    exit 1
fi

# Set LiveKit config from Rust project if not already set
if [ -z "$LIVEKIT_URL" ]; then
    export LIVEKIT_URL="wss://my-agent-t6shkefq.livekit.cloud"
    echo -e "${YELLOW}Using LIVEKIT_URL: $LIVEKIT_URL${NC}"
fi

if [ -z "$LIVEKIT_API_KEY" ]; then
    export LIVEKIT_API_KEY="APIG3jFfastPMAW"
    echo -e "${YELLOW}Using LIVEKIT_API_KEY from config${NC}"
fi

if [ -z "$LIVEKIT_API_SECRET" ]; then
    export LIVEKIT_API_SECRET="7hsvSaqzQPpCmkt1Wj4vRACZljbf31qt3oJ4oc3n4WB"
    echo -e "${YELLOW}Using LIVEKIT_API_SECRET from config${NC}"
fi

echo -e "${GREEN}Configuration:${NC}"
echo "  LLM: Grok 4.1 Fast (OpenRouter)"
echo "  STT: Whisper (local - no API key needed)"
echo "  TTS: Piper (local - no API key needed)"
echo "  LiveKit: $LIVEKIT_URL"
echo ""

# Download Piper voice if not present
PIPER_VOICE_DIR="$HOME/.local/share/piper"
PIPER_VOICE_FILE="$PIPER_VOICE_DIR/en_US-lessac-medium.onnx"
if [ ! -f "$PIPER_VOICE_FILE" ]; then
    echo -e "${YELLOW}Downloading Piper voice (en_US-lessac-medium)...${NC}"
    mkdir -p "$PIPER_VOICE_DIR"
    wget -q -O "$PIPER_VOICE_FILE" "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx"
    wget -q -O "${PIPER_VOICE_FILE}.json" "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx.json"
    echo -e "${GREEN}Piper voice downloaded!${NC}"
fi

# Run the agent
echo -e "${GREEN}Starting LiveKit Agent...${NC}"
cd "$(dirname "$0")"
python3 livekit_agent.py start
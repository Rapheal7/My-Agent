#!/bin/bash
# Voice Agent Control Script
# Usage: voice.sh [on|off|status|test]

set -e

# Colors
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
NC='\033[0m'

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PID_FILE="$SCRIPT_DIR/.voice-agent.pid"
LOG_FILE="$SCRIPT_DIR/voice-agent.log"

# LiveKit configuration
export LIVEKIT_URL="wss://my-agent-t6shkefq.livekit.cloud"
export LIVEKIT_API_KEY="APIG3jFfastPMAW"
export LIVEKIT_API_SECRET="7hsvSaqzQPpCmkt1Wj4vRACZljbf31qt3oJ4oc3n4WB"

start_agent() {
    if [ -f "$PID_FILE" ] && kill -0 $(cat "$PID_FILE") 2>/dev/null; then
        echo -e "${YELLOW}Voice agent already running (PID: $(cat $PID_FILE))${NC}"
        return 1
    fi

    echo -e "${GREEN}Starting voice agent...${NC}"
    cd "$SCRIPT_DIR"

    # Check for OpenRouter API key
    if [ -z "$OPENROUTER_API_KEY" ]; then
        echo -e "${RED}Missing OPENROUTER_API_KEY!${NC}"
        echo "Set it with: export OPENROUTER_API_KEY=your_key"
        return 1
    fi

    # Start in background (explicit room mode for direct connection)
    nohup python3 livekit_agent.py --room voice-session > "$LOG_FILE" 2>&1 &
    echo $! > "$PID_FILE"

    sleep 3

    if kill -0 $(cat "$PID_FILE") 2>/dev/null; then
        echo -e "${GREEN}Voice agent started (PID: $(cat $PID_FILE))${NC}"
        echo "  STT: Faster-Whisper (local)"
        echo "  LLM: Grok 4.1 Fast (OpenRouter)"
        echo "  TTS: Piper (local)"
        echo "  LiveKit: $LIVEKIT_URL"
    else
        echo -e "${RED}Failed to start voice agent${NC}"
        cat "$LOG_FILE"
        rm -f "$PID_FILE"
        return 1
    fi
}

stop_agent() {
    if [ ! -f "$PID_FILE" ]; then
        echo -e "${YELLOW}Voice agent not running${NC}"
        return 0
    fi

    PID=$(cat "$PID_FILE")
    if kill -0 "$PID" 2>/dev/null; then
        echo -e "${YELLOW}Stopping voice agent (PID: $PID)...${NC}"
        kill "$PID" 2>/dev/null || true

        # Also kill any child python processes from the agent
        pkill -f "livekit_agent.py" 2>/dev/null || true

        sleep 1
        echo -e "${GREEN}Voice agent stopped${NC}"
    else
        echo -e "${YELLOW}Voice agent was not running${NC}"
    fi

    rm -f "$PID_FILE"
}

status_agent() {
    echo -e "${BLUE}=== Voice Agent Status ===${NC}"

    # Check HTTPS server
    if lsof -i :3000 >/dev/null 2>&1; then
        echo -e "HTTPS Server: ${GREEN}Running${NC} (port 3000)"
    else
        echo -e "HTTPS Server: ${RED}Not running${NC}"
    fi

    # Check LiveKit agent
    if [ -f "$PID_FILE" ] && kill -0 $(cat "$PID_FILE") 2>/dev/null; then
        echo -e "LiveKit Agent: ${GREEN}Running${NC} (PID: $(cat $PID_FILE))"
        echo "  LiveKit URL: $LIVEKIT_URL"
    else
        echo -e "LiveKit Agent: ${RED}Not running${NC}"
        rm -f "$PID_FILE" 2>/dev/null
    fi

    # Check Piper voice
    PIPER_VOICE="$HOME/.local/share/piper/en_US-lessac-medium.onnx"
    if [ -f "$PIPER_VOICE" ]; then
        echo -e "Piper Voice: ${GREEN}Installed${NC}"
    else
        echo -e "Piper Voice: ${YELLOW}Not installed${NC}"
    fi

    # Check API key
    if [ -n "$OPENROUTER_API_KEY" ]; then
        echo -e "OpenRouter API: ${GREEN}Configured${NC}"
    else
        echo -e "OpenRouter API: ${RED}Not set${NC}"
    fi
}

test_agent() {
    echo -e "${BLUE}=== Testing Voice Agent ===${NC}"

    # Test LiveKit connection
    echo "Testing LiveKit connection..."
    curl -s "https://my-agent-t6shkefq.livekit.cloud" >/dev/null 2>&1 && \
        echo -e "  LiveKit Cloud: ${GREEN}Reachable${NC}" || \
        echo -e "  LiveKit Cloud: ${RED}Unreachable${NC}"

    # Test HTTPS server
    echo "Testing HTTPS server..."
    curl -sk "https://localhost:3000/" >/dev/null 2>&1 && \
        echo -e "  HTTPS Server: ${GREEN}Responding${NC}" || \
        echo -e "  HTTPS Server: ${RED}Not responding${NC}"

    # Test OpenRouter API
    echo "Testing OpenRouter API..."
    if [ -n "$OPENROUTER_API_KEY" ]; then
        curl -s -o /dev/null -w "%{http_code}" \
            -H "Authorization: Bearer $OPENROUTER_API_KEY" \
            "https://openrouter.ai/api/v1/models" 2>/dev/null | grep -q "200" && \
            echo -e "  OpenRouter API: ${GREEN}Valid${NC}" || \
            echo -e "  OpenRouter API: ${YELLOW}Check API key${NC}"
    else
        echo -e "  OpenRouter API: ${RED}No API key${NC}"
    fi
}

case "${1:-status}" in
    on|start)
        start_agent
        ;;
    off|stop)
        stop_agent
        ;;
    status)
        status_agent
        ;;
    test)
        test_agent
        ;;
    restart)
        stop_agent
        sleep 1
        start_agent
        ;;
    logs)
        if [ -f "$LOG_FILE" ]; then
            tail -f "$LOG_FILE"
        else
            echo "No log file found"
        fi
        ;;
    *)
        echo "Usage: $0 {on|off|status|test|restart|logs}"
        echo ""
        echo "Commands:"
        echo "  on, start  - Start the voice agent"
        echo "  off, stop  - Stop the voice agent"
        echo "  status     - Show status of all components"
        echo "  test       - Test connections to all services"
        echo "  restart    - Restart the voice agent"
        echo "  logs       - Follow the agent logs"
        exit 1
        ;;
esac

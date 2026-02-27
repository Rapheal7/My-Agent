#!/bin/bash

# Test script for LiveKit connection

echo "Testing LiveKit Agent AI connection..."
echo "========================================"
echo ""

# Load config values
SERVER_URL=$(cat ~/.config/my-agent/config.toml | grep "server_url" | grep "livekit" | cut -d'"' -f2)
API_KEY=$(cat ~/.config/my-agent/config.toml | grep "api_key" | grep "livekit" | cut -d'"' -f2)
API_SECRET=$(cat ~/.config/my-agent/config.toml | grep "api_secret" | grep "livekit" | cut -d'"' -f2)
ROOM_NAME=$(cat ~/.config/my-agent/config.toml | grep "room_name" | grep "livekit" | cut -d'"' -f2)
VOICE_MODEL=$(cat ~/.config/my-agent/config.toml | grep "voice_model" | grep "livekit" | cut -d'"' -f2)

echo "LiveKit Configuration:"
echo "  Server URL: $SERVER_URL"
echo "  Room: $ROOM_NAME"
echo "  Model: $VOICE_MODEL"
echo ""

# Generate JWT token using Python
echo "Generating JWT token..."
cat > /tmp/generate_jwt.py << 'EOF'
import jwt
import time
import json

# Read config
with open('/tmp/test_jwt_config.txt', 'r') as f:
    api_key = f.readline().strip()
    api_secret = f.readline().strip()
    room_name = f.readline().strip()
    voice_model = f.readline().strip()

# Create claims
now = int(time.time())
claims = {
    "sub": "my-agent-user",
    "room": room_name,
    "name": "My Agent",
    "metadata": "voice-chat",
    "exp": now + 24 * 60 * 60,
    "iss": api_key,
    "nbf": now,
    "agent": True,
    "agent_metadata": f"model:{voice_model}",
    "video_grant": {
        "can_publish": True,
        "can_subscribe": True,
        "can_publish_data": True
    },
    "audio_grant": {
        "can_publish": True,
        "can_subscribe": True
    }
}

# Encode token
token = jwt.encode(claims, api_secret, algorithm='HS256')
print(token)
EOF

echo "$API_KEY" > /tmp/test_jwt_config.txt
echo "$API_SECRET" >> /tmp/test_jwt_config.txt
echo "$ROOM_NAME" >> /tmp/test_jwt_config.txt
echo "$VOICE_MODEL" >> /tmp/test_jwt_config.txt

TOKEN=$(python3 /tmp/generate_jwt.py 2>/dev/null)

if [ -z "$TOKEN" ]; then
    echo "❌ Failed to generate JWT token"
    exit 1
fi

echo "✅ JWT token generated successfully"
echo ""

# Test different endpoint variations
echo "Testing endpoint variations:"
echo "========================================"

# Test 1: Direct server URL with /agent
echo ""
echo "Test 1: Direct server URL + /agent"
URL1="${SERVER_URL}/agent?token=${TOKEN}"
echo "URL: ${URL1:0:80}..."
timeout 10 wscat -c "$URL1" 2>&1 | head -20

# Test 2: Without /agent suffix
echo ""
echo "Test 2: Direct server URL (no /agent)"
URL2="${SERVER_URL}?token=${TOKEN}"
echo "URL: ${URL2:0:80}..."
timeout 10 wscat -c "$URL2" 2>&1 | head -20

# Test 3: Using /ws endpoint (common for websockets)
echo ""
echo "Test 3: /ws endpoint"
URL3="${SERVER_URL}/ws?token=${TOKEN}"
echo "URL: ${URL3:0:80}..."
timeout 10 wscat -c "$URL3" 2>&1 | head -20

# Test 4: Using /api/ws endpoint
echo ""
echo "Test 4: /api/ws endpoint"
URL4="${SERVER_URL}/api/ws?token=${TOKEN}"
echo "URL: ${URL4:0:80}..."
timeout 10 wscat -c "$URL4" 2>&1 | head -20

echo ""
echo "========================================"
echo "Tests completed"

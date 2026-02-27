#!/usr/bin/env python3
"""
Dispatch the LiveKit agent to a specific room.

This script uses the LiveKit Cloud API to explicitly dispatch
the agent to a room, bypassing the need for dashboard configuration.
"""

import argparse
import hmac
import hashlib
import base64
import json
import time
import urllib.request
import urllib.error
import sys

# LiveKit Cloud configuration
LIVEKIT_URL = "wss://my-agent-t6shkefq.livekit.cloud"
API_KEY = "APIG3jFfastPMAW"
API_SECRET = "7hsvSaqzQPpCmkt1Wj4vRACZljbf31qt3oJ4oc3n4WB"
AGENT_NAME = "my-agent-voice"


def create_api_token():
    """Create a JWT token for LiveKit Cloud API access."""
    now = int(time.time())
    payload = {
        "iss": API_KEY,
        "nbf": now,
        "exp": now + 300,
        "video": {
            "roomAdmin": True,
            "roomCreate": True,
            "roomJoin": True,
            "canPublish": True,
            "canSubscribe": True,
            "agent": True,
        }
    }

    def base64url(data):
        return base64.urlsafe_b64encode(json.dumps(data).encode()).decode().rstrip("=")

    header_b64 = base64url({"alg": "HS256", "typ": "JWT"})
    payload_b64 = base64url(payload)
    message = f"{header_b64}.{payload_b64}"
    signature = hmac.new(API_SECRET.encode(), message.encode(), hashlib.sha256).digest()
    sig_b64 = base64.urlsafe_b64encode(signature).decode().rstrip("=")
    return f"{message}.{sig_b64}"


def dispatch_agent(room_name: str) -> bool:
    """Dispatch the agent to a specific room."""
    token = create_api_token()

    # Try the agents dispatch endpoint
    url = "https://api.livekit.cloud/api/agents/dispatch"
    data = json.dumps({
        "room": room_name,
        "agent_name": AGENT_NAME
    }).encode()

    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json"
        },
        method="POST"
    )

    try:
        resp = urllib.request.urlopen(req, timeout=10)
        result = json.loads(resp.read().decode())
        print(f"Agent dispatched successfully to room: {room_name}")
        print(f"Response: {result}")
        return True
    except urllib.error.HTTPError as e:
        error_body = e.read().decode()
        print(f"Dispatch failed ({e.code}): {error_body}")

        # Try alternative endpoint
        alt_url = f"https://api.livekit.cloud/api/rooms/{room_name}/dispatch"
        req2 = urllib.request.Request(
            alt_url,
            data=data,
            headers={
                "Authorization": f"Bearer {token}",
                "Content-Type": "application/json"
            },
            method="POST"
        )
        try:
            resp2 = urllib.request.urlopen(req2, timeout=10)
            result2 = json.loads(resp2.read().decode())
            print(f"Agent dispatched via alternative endpoint: {result2}")
            return True
        except Exception as e2:
            print(f"Alternative dispatch also failed: {e2}")
            return False
    except Exception as e:
        print(f"Error dispatching agent: {e}")
        return False


def create_room_with_agent(room_name: str) -> bool:
    """Create a room with agent dispatch configuration."""
    token = create_api_token()

    url = "https://api.livekit.cloud/api/rooms"
    data = json.dumps({
        "name": room_name,
        "empty_timeout": 300,
        "max_participants": 10,
        # Request agent dispatch
        "agents": [{
            "agent_name": AGENT_NAME
        }]
    }).encode()

    req = urllib.request.Request(
        url,
        data=data,
        headers={
            "Authorization": f"Bearer {token}",
            "Content-Type": "application/json"
        },
        method="POST"
    )

    try:
        resp = urllib.request.urlopen(req, timeout=10)
        result = json.loads(resp.read().decode())
        print(f"Room created with agent dispatch: {room_name}")
        print(f"Response: {result}")
        return True
    except urllib.error.HTTPError as e:
        error_body = e.read().decode()
        print(f"Room creation failed ({e.code}): {error_body}")
        return False
    except Exception as e:
        print(f"Error creating room: {e}")
        return False


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description="Dispatch LiveKit agent to a room")
    parser.add_argument("room", help="Room name to dispatch agent to")
    parser.add_argument("--create", action="store_true", help="Create room with agent dispatch")

    args = parser.parse_args()

    if args.create:
        success = create_room_with_agent(args.room)
    else:
        success = dispatch_agent(args.room)

    sys.exit(0 if success else 1)

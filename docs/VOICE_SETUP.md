# My Agent - Voice Assistant Setup

A fully functional voice assistant using **free AI models** - no ElevenLabs, no OpenAI API costs.

## Architecture

```
┌─────────────────┐      WebRTC       ┌─────────────────┐
│   Browser       │ ◄────────────────► │  LiveKit Cloud  │
│  (Your Phone/PC)│                    │   (Signaling)   │
└─────────────────┘                    └────────┬────────┘
       │                                        │
       │                                        │
       │           ┌─────────────────┐         │
       └──────────►│  Python Agent   │◄────────┘
                   │   (Your PC)     │
                   └────────┬────────┘
                            │
        ┌───────────────────┼───────────────────┐
        ▼                   ▼                   ▼
   ┌─────────┐        ┌──────────┐       ┌──────────┐
   │ Whisper │        │   Grok   │       │  Piper   │
   │  (STT)  │        │   (LLM)  │       │  (TTS)   │
   │  Local  │        │  Free*   │       │  Local   │
   └─────────┘        └──────────┘       └──────────┘
```

\* Grok 4.1 Fast via OpenRouter (free tier available)

## Components

| Component | Technology | Cost | Location |
|-----------|------------|------|----------|
| **STT** (Speech-to-Text) | Faster-Whisper | Free | Local |
| **LLM** (Brain) | Grok 4.1 Fast | Free* | Cloud (OpenRouter) |
| **TTS** (Text-to-Speech) | Piper | Free | Local |
| **VAD** (Voice Detection) | Silero | Free | Local |
| **Transport** | LiveKit | Free tier | Cloud |

## Quick Start

### 1. Prerequisites

```bash
# Install Python dependencies
pip install livekit livekit-agents faster-whisper piper-tts

# Get OpenRouter API key (free)
# Visit: https://openrouter.ai/keys
```

### 2. Configuration

Edit `.env` file in the project root:

```env
LIVEKIT_API_KEY=APIG3jFfastPMAW
LIVEKIT_API_SECRET=7hsvSaqzQPpCmkt1Wj4vRACZljbf31qt3oJ4oc3n4WB
LIVEKIT_URL=wss://my-agent-t6shkefq.livekit.cloud
OPENROUTER_API_KEY=sk-or-v1-your_key_here
```

### 3. Start the System

```bash
./start-voice-complete.sh
```

This will:
1. Start the HTTP server on port 8080
2. Start the LiveKit agent
3. Print access URLs

### 4. Access the Voice Client

Open in your browser:
- **Local**: http://localhost:8080/voice-client.html
- **Network**: http://YOUR_IP:8080/voice-client.html (for phone access)

### 5. Using the Voice Client

1. Tap the **microphone button** (🎤) to connect
2. Grant microphone permissions when prompted
3. Wait for "Connected!" status
4. **Start speaking** - the agent will respond

## Files Overview

| File | Purpose |
|------|---------|
| `livekit_agent_v2.py` | Main agent with reconnection logic |
| `static/voice-client.html` | Browser client (mobile-friendly) |
| `simple_server.py` | HTTP server for serving the client |
| `start-voice-complete.sh` | One-script startup |

## How It Works

### Connection Flow

1. **Browser** generates a JWT token using LiveKit credentials
2. **Browser** connects to LiveKit Cloud WebSocket
3. **Browser** publishes microphone audio
4. **Agent** (already running) detects new participant
5. **Agent** subscribes to browser's audio track
6. **Agent** processes audio through STT → LLM → TTS
7. **Agent** publishes response audio
8. **Browser** plays agent's audio response

### Audio Pipeline

```
Your Voice ──► Browser ──► LiveKit ──► Agent
                                           │
    ┌──────────────────────────────────────┘
    ▼
Faster-Whisper (STT) ──► Text
                              │
                              ▼
                         Grok 4.1 Fast (LLM)
                              │
                              ▼
                         Text Response
                              │
                              ▼
                         Piper (TTS)
                              │
                              ▼
    ┌──────────────────────────────────────┐
    ▼                                      │
Agent ──► LiveKit ──► Browser ──► Speakers │
```

## Troubleshooting

### "Connection failed" or "401 Unauthorized"

- Check your `.env` file has correct credentials
- Verify `OPENROUTER_API_KEY` is set
- Try restarting the agent: `python3 livekit_agent_v2.py`

### "No audio from agent"

1. Click **"Test Audio"** button to verify your speakers work
2. Check browser console for errors (F12 → Console)
3. Try refreshing the page
4. Ensure microphone permissions are granted

### Agent not joining

1. Check agent is running: `ps aux | grep livekit_agent`
2. Check logs: `tail -f voice-agent.log`
3. Restart the agent

### Poor audio quality

- Use headphones to prevent echo
- Ensure you're in a quiet environment
- Check your internet connection

## Advanced Usage

### Custom Room Name

```bash
python3 livekit_agent_v2.py --room my-custom-room
```

Then modify the browser client to use the same room.

### Running Manually

**Terminal 1 - HTTP Server:**
```bash
python3 simple_server.py 8080
```

**Terminal 2 - Agent:**
```bash
python3 livekit_agent_v2.py --room voice-session
```

**Browser:**
Open http://localhost:8080/voice-client.html

## Mobile Access

For phone access, you need to:

1. Find your computer's IP: `hostname -I`
2. Access via: `http://YOUR_IP:8080/voice-client.html`
3. Both devices must be on the same network

For remote access over the internet, use:
- **Cloudflare Tunnel** (recommended)
- **ngrok**
- **Tailscale**

## Model Details

### Faster-Whisper (STT)
- Model: `base` (can upgrade to `small`, `medium`, `large-v3`)
- Runs on CPU with int8 quantization
- Supports 99 languages
- Real-time transcription

### Grok 4.1 Fast (LLM)
- Via OpenRouter (free tier: 20 requests/minute)
- Fast responses (~100-300ms)
- Good for conversational AI

### Piper (TTS)
- Voice: `en_US-lessac-medium`
- Sample rate: 22050 Hz
- Very fast synthesis (~real-time)
- Natural sounding

## Cost Comparison

| Service | ElevenLabs | This Setup |
|---------|------------|------------|
| STT | $0.02/min | **Free** (local) |
| LLM | $0.03-0.15/min | **Free** (OpenRouter) |
| TTS | $0.10-0.30/min | **Free** (local) |
| **Monthly** (1hr/day) | **$100-400** | **$0** |

## Security Notes

- API keys in `.env` are for your local use only
- LiveKit tokens are generated client-side with short expiry (1 hour)
- No audio data is stored permanently
- All WebRTC connections are encrypted

## Next Steps

1. **Customize the system prompt** in `livekit_agent_v2.py`
2. **Add more voices** by downloading different Piper models
3. **Upgrade Whisper** to `large-v3` for better accuracy
4. **Add function calling** for home automation, search, etc.

## Support

- LiveKit Docs: https://docs.livekit.io
- OpenRouter: https://openrouter.ai/docs
- Faster-Whisper: https://github.com/SYSTRAN/faster-whisper

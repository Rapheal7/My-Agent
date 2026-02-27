#!/usr/bin/env python3
"""
Unified Voice Assistant Server - HTTP + WebSocket on same port
"""

import asyncio
import os
import sys
import io
import json
import logging
from aiohttp import web, WSCloseCode
import aiohttp

# Setup logging
logging.basicConfig(level=logging.INFO, format='%(levelname)s:%(name)s:%(message)s')
logger = logging.getLogger(__name__)

# Get API key from env
OPENROUTER_API_KEY = os.getenv("OPENROUTER_API_KEY", "")

# Agent API configuration - connect to Rust agent
AGENT_API_URL = os.getenv("AGENT_API_URL", "http://localhost:3000")
AGENT_API_KEY = os.getenv("AGENT_API_KEY", "")

# Connected WebSocket clients
connected_clients = set()

async def chat_with_agent(text: str) -> str:
    """Send message to agent API and get response with full tool support"""
    try:
        import aiohttp

        async with aiohttp.ClientSession() as session:
            headers = {"Content-Type": "text/plain"}
            if AGENT_API_KEY:
                headers["Authorization"] = f"Bearer {AGENT_API_KEY}"

            # Call the agent's chat API
            async with session.post(
                f"{AGENT_API_URL}/api/chat",
                data=text,
                headers=headers,
                timeout=aiohttp.ClientTimeout(total=60)
            ) as response:
                if response.status == 200:
                    reply = await response.text()
                    logger.info(f"Agent reply: {reply[:100]}...")
                    return reply
                else:
                    error_text = await response.text()
                    logger.error(f"Agent API error {response.status}: {error_text}")
                    # Fallback to direct LLM if agent is unavailable
                    return await chat_with_llm_fallback(text)
    except Exception as e:
        logger.error(f"Agent connection error: {e}")
        # Fallback to direct LLM
        return await chat_with_llm_fallback(text)

async def speech_to_text(audio_bytes: bytes) -> str:
    """Convert audio to text using faster_whisper (English only)"""
    try:
        from faster_whisper import WhisperModel

        # Use small model for better accuracy, force English
        model = WhisperModel("small", device="cpu", compute_type="int8")
        audio_buffer = io.BytesIO(audio_bytes)
        # Force English language to prevent Mandarin detection
        segments, info = model.transcribe(audio_buffer, beam_size=5, language="en")
        text = " ".join([seg.text for seg in segments])
        logger.info(f"STT result: {text}")
        return text.strip()
    except Exception as e:
        logger.error(f"STT error: {e}")
        return ""

def clean_text_for_tts(text: str) -> str:
    """Remove emojis, markdown, and special characters that TTS shouldn't read"""
    import re

    # Remove emojis - pattern matches most emoji ranges
    emoji_pattern = re.compile(
        "["
        "\U0001F600-\U0001F64F"  # emoticons
        "\U0001F300-\U0001F5FF"  # symbols & pictographs
        "\U0001F680-\U0001F6FF"  # transport & map symbols
        "\U0001F1E0-\U0001F1FF"  # flags
        "\U00002702-\U000027B0"
        "\U000024C2-\U0001F251"
        "\U0001F900-\U0001F9FF"  # supplemental symbols
        "\U0001FA00-\U0001FA6F"  # chess symbols
        "\U0001FA70-\U0001FAFF"  # symbols and pictographs extended-a
        "\U00002600-\U000026FF"  # miscellaneous symbols
        "\U00002500-\U00002BEF"  # other symbols
        "\U00002300-\U000023FF"  # miscellaneous technical
        "]+",
        flags=re.UNICODE
    )
    text = emoji_pattern.sub('', text)

    # Remove ALL markdown formatting more aggressively
    text = re.sub(r'\*\*\*([^*]+)\*\*\*', r'\1', text)  # bold+italic
    text = re.sub(r'\*\*([^*]+)\*\*', r'\1', text)      # bold
    text = re.sub(r'\*([^*]+)\*', r'\1', text)          # italic
    text = re.sub(r'__([^_]+)__', r'\1', text)          # underline
    text = re.sub(r'_([^_]+)_', r'\1', text)            # italic
    text = re.sub(r'`([^`]+)`', r'\1', text)            # inline code
    text = re.sub(r'```[\s\S]*?```', '', text)          # code blocks
    text = re.sub(r'\[([^\]]+)\]\([^)]+\)', r'\1', text)  # markdown links
    text = re.sub(r'\[([^\]]+)\]\[[^\]]*\]', r'\1', text)  # reference links
    text = re.sub(r'#+\s+', '', text)                   # headers
    text = re.sub(r'>\s*', '', text)                    # blockquotes
    text = re.sub(r'[-*]\s+', '', text)                 # list bullets
    text = re.sub(r'\d+\.\s+', '', text)                # numbered lists
    text = re.sub(r'~~([^~]+)~~', r'\1', text)          # strikethrough

    # Remove URLs (TTS shouldn't read them)
    text = re.sub(r'https?://\S+', '', text)

    # Remove common formatting characters but keep the content
    text = text.replace('**', '').replace('*', '').replace('__', '').replace('_', '')
    text = text.replace('`', '').replace('~', '').replace('#', '').replace('>', '')

    # Clean up extra whitespace
    text = ' '.join(text.split())

    return text.strip()

async def text_to_speech(text: str) -> bytes:
    """Convert text to speech using Piper (neural TTS)"""
    import subprocess
    import tempfile
    import shutil
    import asyncio

    # Clean text before TTS
    text = clean_text_for_tts(text)

    if not text:
        return b""

    try:
        # Find piper executable
        piper_path = shutil.which("piper") or os.path.expanduser("~/.local/bin/piper")

        # Find voice model
        voice_dir = os.path.expanduser("~/.local/share/piper")
        if not os.path.exists(voice_dir):
            voice_dir = os.path.expanduser("~/piper-voices")

        # Look for a voice model
        voice_model = None
        for f in os.listdir(voice_dir) if os.path.exists(voice_dir) else []:
            if f.endswith('.onnx') and not f.endswith('.json'):
                voice_model = os.path.join(voice_dir, f)
                break

        if not voice_model or not os.path.exists(piper_path):
            logger.error(f"Piper not found at {piper_path} or no voice model in {voice_dir}")
            return b""

        # Create temp files
        with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False, encoding='utf-8') as f:
            f.write(text)
            f.flush()
            text_path = f.name

        with tempfile.NamedTemporaryFile(suffix='.wav', delete=False) as f:
            wav_path = f.name

        logger.info(f"TTS input text: {repr(text[:100])}")

        # Run piper in executor to not block event loop
        def run_piper():
            result = subprocess.run(
                [piper_path, '--model', voice_model, '--input_file', text_path, '--output_file', wav_path],
                capture_output=True,
                text=True
            )
            return result

        loop = asyncio.get_event_loop()
        result = await loop.run_in_executor(None, run_piper)

        # Clean up text file
        os.unlink(text_path)

        # Check if output file was created (piper may exit with error due to GPU warnings but still work)
        if not os.path.exists(wav_path) or os.path.getsize(wav_path) == 0:
            logger.error(f"Piper failed to create output: {result.stderr}")
            return b""

        # Read the generated audio
        with open(wav_path, 'rb') as f:
            audio_bytes = f.read()

        os.unlink(wav_path)
        logger.info(f"Piper TTS generated {len(audio_bytes)} bytes")
        return audio_bytes

    except Exception as e:
        logger.error(f"TTS error: {e}")
        return b""

async def chat_with_llm_fallback(text: str) -> str:
    """Fallback: Get response from LLM via OpenRouter (when agent is unavailable)"""
    try:
        import openai

        client = openai.OpenAI(
            api_key=OPENROUTER_API_KEY,
            base_url="https://openrouter.ai/api/v1"
        )

        response = client.chat.completions.create(
            model="x-ai/grok-4.1-fast",
            messages=[
                {"role": "system", "content": "You are a helpful voice assistant. Keep responses short and conversational."},
                {"role": "user", "content": text}
            ],
            max_tokens=200
        )

        reply = response.choices[0].message.content
        logger.info(f"LLM fallback reply: {reply[:50]}...")
        return reply
    except Exception as e:
        logger.error(f"LLM fallback error: {e}")
        return "Sorry, I had trouble processing that."

async def process_voice_command(audio_bytes: bytes) -> tuple:
    """Process voice input: STT -> Agent -> TTS"""
    logger.info("Processing voice command...")

    text = await speech_to_text(audio_bytes)
    if not text:
        return "I didn't catch that.", b""

    # Send to agent for full tool/orchestration support
    reply = await chat_with_agent(text)
    audio_reply = await text_to_speech(reply)

    return reply, audio_reply

async def websocket_handler(request):
    """Handle WebSocket connections"""
    ws = web.WebSocketResponse()
    await ws.prepare(request)

    client_id = id(ws)
    logger.info(f"Client {client_id} connected")
    connected_clients.add(ws)

    try:
        async for msg in ws:
            if msg.type == aiohttp.WSMsgType.TEXT:
                try:
                    data = json.loads(msg.data)
                    msg_type = data.get("type")

                    if msg_type in ("audio", "voice"):
                        import base64

                        audio_b64 = data.get("audio", data.get("audio_data", ""))
                        audio_bytes = base64.b64decode(audio_b64)
                        logger.info(f"Received {len(audio_bytes)} bytes of audio")

                        await ws.send_json({"type": "status", "text": "Processing..."})

                        reply, audio_reply = await process_voice_command(audio_bytes)

                        response = {
                            "type": "response",
                            "text": reply,
                            "transcript": data.get("transcript", ""),
                            "response": reply
                        }

                        if audio_reply:
                            response["audio"] = base64.b64encode(audio_reply).decode()

                        await ws.send_json(response)

                    elif msg_type == "ping":
                        await ws.send_json({"type": "pong"})

                    elif msg_type == "text":
                        text = data.get("text", "")
                        reply = await chat_with_agent(text)
                        await ws.send_json({
                            "type": "response",
                            "text": reply,
                            "response": reply
                        })

                except Exception as e:
                    logger.error(f"Error handling message: {e}")
                    await ws.send_json({"type": "error", "text": str(e)})

            elif msg.type == aiohttp.WSMsgType.ERROR:
                logger.error(f"WebSocket error: {ws.exception()}")

    finally:
        connected_clients.discard(ws)
        logger.info(f"Client {client_id} disconnected")

    return ws

async def index_handler(request):
    """Serve the main HTML file"""
    html_content = open(os.path.join(os.path.dirname(__file__), 'simple-voice.html')).read()
    return web.Response(text=html_content, content_type='text/html')

async def simple_voice_handler(request):
    """Serve the simple voice HTML file"""
    html_content = open(os.path.join(os.path.dirname(__file__), 'simple-voice.html')).read()
    return web.Response(text=html_content, content_type='text/html')

async def on_shutdown(app):
    """Clean up on shutdown"""
    for ws in connected_clients:
        await ws.close(code=WSCloseCode.GOING_AWAY, message='Server shutdown')

async def main():
    import argparse
    import ssl

    parser = argparse.ArgumentParser(description='Voice Assistant Server')
    parser.add_argument('--port', type=int, default=8765, help='Port to listen on')
    parser.add_argument('--https', action='store_true', help='Enable HTTPS')
    parser.add_argument('--cert', type=str, help='SSL certificate file')
    parser.add_argument('--key', type=str, help='SSL key file')
    args = parser.parse_args()

    app = web.Application()
    app.router.add_get('/', index_handler)
    app.router.add_get('/simple-voice', simple_voice_handler)
    app.router.add_get('/ws', websocket_handler)
    app.router.add_get('/voice-ws', websocket_handler)
    app.router.add_static('/', path=os.path.dirname(__file__), show_index=False)

    app.on_shutdown.append(on_shutdown)

    ssl_context = None
    if args.https and args.cert and args.key:
        ssl_context = ssl.create_default_context(ssl.Purpose.CLIENT_AUTH)
        ssl_context.load_cert_chain(args.cert, args.key)
        logger.info(f"HTTPS enabled")

    runner = web.AppRunner(app)
    await runner.setup()

    site = web.TCPSite(runner, '0.0.0.0', args.port, ssl_context=ssl_context)
    await site.start()

    protocol = 'https' if ssl_context else 'http'
    logger.info(f"=" * 60)
    logger.info(f"Voice Server running on {protocol}://0.0.0.0:{args.port}")
    logger.info(f"Access: {protocol}://localhost:{args.port}")
    logger.info(f"=" * 60)

    # Run forever
    while True:
        await asyncio.sleep(3600)

if __name__ == "__main__":
    print("=" * 60)
    print("Voice Assistant Server")
    print("=" * 60)
    asyncio.run(main())

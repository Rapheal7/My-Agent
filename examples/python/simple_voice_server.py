#!/usr/bin/env python3
"""
Simple Voice Assistant Server - Combined HTTP + WebSocket
Serves the voice client HTML and handles WebSocket connections
"""

import asyncio
import os
import sys
import io
import json
import logging
import websockets
from http.server import HTTPServer, SimpleHTTPRequestHandler
from threading import Thread
import ssl

# Setup logging
logging.basicConfig(level=logging.INFO, format='%(levelname)s:%(name)s:%(message)s')
logger = logging.getLogger(__name__)

# Get API key from env
OPENROUTER_API_KEY = os.getenv("OPENROUTER_API_KEY", "")
if not OPENROUTER_API_KEY:
    logger.error("OPENROUTER_API_KEY not set!")
    # Don't exit - we can still serve the HTML

# Audio settings
SAMPLE_RATE = 22050

# Connected clients
connected_clients = set()

async def speech_to_text(audio_bytes: bytes) -> str:
    """Convert audio to text using faster_whisper"""
    try:
        from faster_whisper import WhisperModel

        # Use a small model for speed
        model = WhisperModel("tiny", device="cpu", compute_type="int8")

        # Write audio to buffer
        audio_buffer = io.BytesIO(audio_bytes)

        # Transcribe
        segments, info = model.transcribe(audio_buffer, beam_size=5)

        text = " ".join([seg.text for seg in segments])
        logger.info(f"STT result: {text}")
        return text.strip()

    except Exception as e:
        logger.error(f"STT error: {e}")
        return ""

async def text_to_speech(text: str) -> bytes:
    """Convert text to speech using a TTS service"""
    try:
        # Try using pyttsx3 first (local)
        import pyttsx3
        import tempfile

        engine = pyttsx3.init()
        engine.setProperty('rate', 150)

        # Save to temporary file
        with tempfile.NamedTemporaryFile(suffix='.wav', delete=False) as f:
            temp_path = f.name

        engine.save_to_file(text, temp_path)
        engine.runAndWait()

        # Read the audio file
        with open(temp_path, 'rb') as f:
            audio_bytes = f.read()

        os.unlink(temp_path)
        logger.info(f"TTS generated {len(audio_bytes)} bytes using pyttsx3")
        return audio_bytes

    except Exception as e:
        logger.error(f"TTS error: {e}")
        # Return empty bytes - client will just show text
        return b""

async def chat_with_llm(text: str) -> str:
    """Get response from LLM via OpenRouter"""
    try:
        import openai

        client = openai.OpenAI(
            api_key=OPENROUTER_API_KEY,
            base_url="https://openrouter.ai/api/v1"
        )

        response = client.chat.completions.create(
            model="meta-llama/llama-3.1-8b-instruct",
            messages=[
                {"role": "system", "content": "You are a helpful voice assistant. Keep responses short and conversational."},
                {"role": "user", "content": text}
            ],
            max_tokens=200
        )

        reply = response.choices[0].message.content
        logger.info(f"LLM reply: {reply}")
        return reply

    except Exception as e:
        logger.error(f"LLM error: {e}")
        return "Sorry, I had trouble thinking just now."

async def process_voice_command(audio_bytes: bytes) -> tuple:
    """Process voice input: STT -> LLM -> TTS"""
    logger.info("Processing voice command...")

    # Step 1: Speech to Text
    text = await speech_to_text(audio_bytes)
    if not text:
        return "I didn't catch that.", b""

    logger.info(f"User said: {text}")

    # Step 2: Get LLM response
    reply = await chat_with_llm(text)

    # Step 3: Text to Speech
    audio_reply = await text_to_speech(reply)

    return reply, audio_reply

async def handle_websocket(websocket, path):
    """Handle WebSocket connections"""
    client_id = id(websocket)
    logger.info(f"Client {client_id} connected")
    connected_clients.add(websocket)

    try:
        async for message in websocket:
            try:
                data = json.loads(message)
                msg_type = data.get("type")

                if msg_type == "audio" or msg_type == "voice":
                    import base64

                    # Decode audio
                    audio_b64 = data.get("audio", data.get("audio_data", ""))
                    audio_bytes = base64.b64decode(audio_b64)

                    logger.info(f"Received {len(audio_bytes)} bytes of audio")

                    # Send processing status
                    await websocket.send(json.dumps({
                        "type": "status",
                        "text": "Processing..."
                    }))

                    # Process
                    reply, audio_reply = await process_voice_command(audio_bytes)

                    # Encode response
                    response_data = {
                        "type": "response",
                        "text": reply,
                        "transcript": data.get("transcript", ""),
                        "response": reply
                    }

                    if audio_reply:
                        audio_b64 = base64.b64encode(audio_reply).decode()
                        response_data["audio"] = audio_b64

                    await websocket.send(json.dumps(response_data))
                    logger.info(f"Sent response: {reply[:50]}...")

                elif msg_type == "ping":
                    await websocket.send(json.dumps({"type": "pong"}))

                elif msg_type == "text":
                    # Handle text-only messages
                    text = data.get("text", "")
                    reply = await chat_with_llm(text)
                    await websocket.send(json.dumps({
                        "type": "response",
                        "text": reply,
                        "response": reply
                    }))

            except Exception as e:
                logger.error(f"Error handling message: {e}")
                await websocket.send(json.dumps({
                    "type": "error",
                    "text": str(e)
                }))

    except websockets.exceptions.ConnectionClosed:
        logger.info(f"Client {client_id} disconnected")
    finally:
        connected_clients.discard(websocket)

def serve_http(port, directory):
    """Serve HTTP files"""
    class Handler(SimpleHTTPRequestHandler):
        def __init__(self, *args, **kwargs):
            super().__init__(*args, directory=directory, **kwargs)

        def end_headers(self):
            self.send_header('Access-Control-Allow-Origin', '*')
            self.send_header('Access-Control-Allow-Methods', 'GET, POST, OPTIONS')
            self.send_header('Access-Control-Allow-Headers', 'Content-Type')
            super().end_headers()

        def do_OPTIONS(self):
            self.send_response(200)
            self.end_headers()

        def log_message(self, format, *args):
            logger.info(format % args)

    httpd = HTTPServer(('0.0.0.0', port), Handler)
    logger.info(f"HTTP Server running on http://0.0.0.0:{port}")
    httpd.serve_forever()

async def main():
    """Main function - start both HTTP and WebSocket servers"""
    import argparse

    parser = argparse.ArgumentParser(description='Simple Voice Server')
    parser.add_argument('--http-port', type=int, default=8766, help='HTTP port')
    parser.add_argument('--ws-port', type=int, default=8765, help='WebSocket port')
    parser.add_argument('--directory', type=str, default='.', help='Directory to serve')
    parser.add_argument('--https', action='store_true', help='Enable HTTPS')
    parser.add_argument('--cert', type=str, help='SSL certificate file')
    parser.add_argument('--key', type=str, help='SSL key file')
    args = parser.parse_args()

    # Get the directory containing this script
    script_dir = os.path.dirname(os.path.abspath(__file__))
    serve_dir = os.path.join(script_dir, args.directory)

    # Ensure the HTML file exists
    html_path = os.path.join(serve_dir, 'simple-voice.html')
    if not os.path.exists(html_path):
        logger.warning(f"{html_path} not found!")

    # Start HTTP server in a thread
    http_thread = Thread(target=serve_http, args=(args.http_port, serve_dir), daemon=True)
    http_thread.start()

    # Start WebSocket server
    logger.info(f"Starting WebSocket server on port {args.ws_port}...")

    if args.https and args.cert and args.key:
        ssl_context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ssl_context.load_cert_chain(args.cert, args.key)
        logger.info(f"HTTPS enabled with cert: {args.cert}")
    else:
        ssl_context = None

    async with websockets.serve(handle_websocket, '0.0.0.0', args.ws_port, ssl=ssl_context):
        logger.info(f"WebSocket server ready on ws{'s' if ssl_context else ''}://0.0.0.0:{args.ws_port}")
        logger.info(f"HTTP server running on http://0.0.0.0:{args.http_port}")
        logger.info(f"Open: http://localhost:{args.http_port}/simple-voice.html")
        await asyncio.Future()  # Run forever

if __name__ == "__main__":
    print("=" * 60)
    print("Simple Voice Assistant Server")
    print("=" * 60)
    asyncio.run(main())

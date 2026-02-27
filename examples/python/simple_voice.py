#!/usr/bin/env python3
"""
Simple Voice Assistant - STT + LLM + TTS
Runs locally without LiveKit cloud
"""

import asyncio
import os
import sys
import io
import json
import logging

# Setup logging
logging.basicConfig(level=logging.INFO, format='%(levelname)s:%(name)s:%(message)s')
logger = logging.getLogger(__name__)

# Get API key from env
OPENROUTER_API_KEY = os.getenv("OPENROUTER_API_KEY", "")
if not OPENROUTER_API_KEY:
    logger.error("OPENROUTER_API_KEY not set!")
    sys.exit(1)

# Audio settings
SAMPLE_RATE = 22050

async def speech_to_text(audio_bytes: bytes) -> str:
    """Convert audio to text using Faster-Whisper"""
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
    """Convert text to speech using Piper"""
    try:
        from piper import PiperVoice

        # Load voice
        voice = PiperVoice.load("en_US-lessac-medium")

        # Synthesize
        audio_buffer = io.BytesIO()
        for chunk in voice.synthesize_stream_raw(text):
            audio_buffer.write(chunk)

        audio_bytes = audio_buffer.getvalue()
        logger.info(f"TTS generated {len(audio_bytes)} bytes")
        return audio_bytes

    except Exception as e:
        logger.error(f"TTS error: {e}")
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

async def simple_websocket_server():
    """Simple WebSocket server for voice I/O"""
    import websockets

    logger.info("Starting simple voice assistant on port 8765...")

    async with websockets.serve(handle_client, "0.0.0.0", 8765):
        logger.info("Server ready!")
        await asyncio.Future()

async def handle_client(websocket):
    """Handle a client connection"""
    client_id = id(websocket)
    logger.info(f"Client {client_id} connected")

    try:
        async for message in websocket:
            try:
                data = json.loads(message)

                if data.get("type") == "audio":
                    import base64

                    # Decode audio
                    audio_b64 = data.get("audio", "")
                    audio_bytes = base64.b64decode(audio_b64)

                    # Process
                    reply, audio_reply = await process_voice_command(audio_bytes)

                    # Encode response
                    if audio_reply:
                        audio_b64 = base64.b64encode(audio_reply).decode()
                    else:
                        audio_b64 = ""

                    await websocket.send(json.dumps({
                        "type": "response",
                        "text": reply,
                        "audio": audio_b64
                    }))

                elif data.get("type") == "ping":
                    await websocket.send(json.dumps({"type": "pong"}))

            except Exception as e:
                logger.error(f"Error: {e}")

    except Exception as e:
        logger.info(f"Client {client_id} disconnected")

if __name__ == "__main__":
    print("=" * 50)
    print("Simple Voice Assistant - No LiveKit!")
    print("=" * 50)

    asyncio.run(simple_websocket_server())

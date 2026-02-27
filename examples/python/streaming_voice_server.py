#!/usr/bin/env python3
"""
Streaming Voice Assistant Server with Interruption and Backchanneling

Features:
- Continuous audio streaming (not turn-based)
- Interrupt detection while TTS plays
- Barge-in support (talk over AI)
- Backchanneling ("uh-huh", "right" during speech)
- Tap-to-stop immediately

Architecture:
- Bidirectional WebSocket audio stream
- Concurrent listen + speak states
- Real-time VAD on server
- Interrupt detection
"""

import asyncio
import os
import sys
import io
import json
import logging
import time
import threading
from aiohttp import web, WSCloseCode
import aiohttp
import numpy as np

# Setup logging
logging.basicConfig(level=logging.INFO, format='%(levelname)s:%(name)s:%(message)s')
logger = logging.getLogger(__name__)

# Get API key from env
OPENROUTER_API_KEY = os.getenv("OPENROUTER_API_KEY", "")
AGENT_API_URL = os.getenv("AGENT_API_URL", "http://localhost:3000")

class StreamingVoiceSession:
    """Manages a streaming voice conversation with interruption support"""

    def __init__(self, ws):
        self.ws = ws
        self.is_listening = False
        self.is_speaking = False
        self.is_processing = False
        self.should_interrupt = False
        self.audio_buffer = []
        self.vad_buffer = []

        # Conversation state
        self.conversation_history = []
        self.current_utterance = []
        self.last_speech_time = 0
        self.silence_start = None

        # TTS playback
        self.current_tts_task = None
        self.tts_audio_queue = asyncio.Queue()
        self.interrupt_event = asyncio.Event()

    async def send_status(self, status_type, message):
        """Send status update to client"""
        try:
            await self.ws.send_json({
                "type": "status",
                "status": status_type,
                "message": message
            })
        except:
            pass

    async def send_backchannel(self, text):
        """Send a short backchannel acknowledgment"""
        backchannels = ["uh-huh", "right", "I see", "okay", "mm-hmm"]
        import random
        bc = random.choice(backchannels)
        try:
            await self.ws.send_json({
                "type": "backchannel",
                "text": bc
            })
            logger.info(f"Sent backchannel: {bc}")
        except:
            pass

    async def process_streaming_audio(self, audio_chunk):
        """Process incoming audio chunks in real-time"""
        # Add to VAD buffer for real-time detection
        self.vad_buffer.append(audio_chunk)

        # Keep only last 2 seconds for VAD
        max_samples = int(16000 * 2)  # 2 seconds at 16kHz
        if len(self.vad_buffer) > max_samples:
            self.vad_buffer = self.vad_buffer[-max_samples:]

        # Simple energy-based VAD
        if len(self.vad_buffer) >= 1600:  # 100ms minimum
            energy = self._calculate_energy(self.vad_buffer[-1600:])

            # Detect speech
            if energy > 0.02:  # Speech threshold
                if not self.is_listening:
                    self.is_listening = True
                    self.silence_start = None
                    await self.send_status("listening", "Hearing you...")

                    # INTERRUPT: If AI is speaking, stop it
                    if self.is_speaking:
                        await self.interrupt_speaking()

                self.current_utterance.append(audio_chunk)
                self.last_speech_time = time.time()
            else:
                # Silence detected
                if self.is_listening:
                    if self.silence_start is None:
                        self.silence_start = time.time()
                    elif time.time() - self.silence_start > 0.8:  # 800ms silence
                        # End of utterance, process it
                        if len(self.current_utterance) > int(16000 * 0.5):  # Min 500ms
                            asyncio.create_task(self.process_utterance())
                        self.is_listening = False
                        self.current_utterance = []
                        self.silence_start = None

    def _calculate_energy(self, samples):
        """Calculate RMS energy of audio samples"""
        if len(samples) == 0:
            return 0
        samples = np.array(samples)
        return np.sqrt(np.mean(samples ** 2))

    async def interrupt_speaking(self):
        """Interrupt current AI speech"""
        logger.info("Interrupt detected! Stopping current speech...")
        self.should_interrupt = True
        self.interrupt_event.set()
        self.is_speaking = False
        await self.send_status("interrupted", "Interrupted")

        # Send stop command to client
        try:
            await self.ws.send_json({
                "type": "stop_audio",
                "reason": "interrupt"
            })
        except:
            pass

    async def process_utterance(self):
        """Process a completed user utterance"""
        if self.is_processing:
            return

        self.is_processing = True
        await self.send_status("processing", "Processing...")

        try:
            # Convert audio to text
            audio_data = np.concatenate(self.current_utterance)
            text = await self.speech_to_text(audio_data)

            if not text:
                self.is_processing = False
                return

            logger.info(f"User: {text}")
            await self.send_status("transcription", text)

            # Get response from agent
            response = await self.chat_with_agent(text)

            if response:
                logger.info(f"Agent: {response}")
                await self.speak_response(response)

        except Exception as e:
            logger.error(f"Error processing utterance: {e}")
        finally:
            self.is_processing = False

    async def speech_to_text(self, audio_data):
        """Convert speech to text"""
        from faster_whisper import WhisperModel

        try:
            model = WhisperModel("small", device="cpu", compute_type="int8")

            # Convert to bytes
            audio_bytes = (audio_data * 32767).astype(np.int16).tobytes()
            audio_buffer = io.BytesIO(audio_bytes)

            segments, info = model.transcribe(audio_buffer, language="en")
            text = " ".join([seg.text for seg in segments])
            return text.strip()
        except Exception as e:
            logger.error(f"STT error: {e}")
            return ""

    async def chat_with_agent(self, text):
        """Send message to agent and get response"""
        try:
            async with aiohttp.ClientSession() as session:
                async with session.post(
                    f"{AGENT_API_URL}/api/chat",
                    data=text,
                    headers={"Content-Type": "text/plain"},
                    timeout=aiohttp.ClientTimeout(total=30)
                ) as response:
                    if response.status == 200:
                        return await response.text()
        except Exception as e:
            logger.error(f"Agent error: {e}")
        return "I'm sorry, I didn't catch that."

    async def speak_response(self, text):
        """Convert text to speech and send audio chunks"""
        self.is_speaking = True
        self.should_interrupt = False
        self.interrupt_event.clear()

        await self.send_status("speaking", "Speaking...")

        try:
            # Generate TTS in background
            tts_task = asyncio.create_task(self._generate_tts_chunks(text))

            # Stream chunks as they become available
            while not tts_task.done() or not self.tts_audio_queue.empty():
                if self.should_interrupt:
                    logger.info("TTS interrupted")
                    break

                try:
                    chunk = await asyncio.wait_for(
                        self.tts_audio_queue.get(),
                        timeout=0.1
                    )

                    # Send audio chunk to client
                    import base64
                    await self.ws.send_json({
                        "type": "audio_chunk",
                        "data": base64.b64encode(chunk).decode(),
                        "final": False
                    })
                except asyncio.TimeoutError:
                    continue

            # Send final marker
            await self.ws.send_json({"type": "audio_chunk", "final": True})

        except Exception as e:
            logger.error(f"TTS error: {e}")
        finally:
            self.is_speaking = False
            await self.send_status("idle", "Tap mic to speak")

    async def _generate_tts_chunks(self, text):
        """Generate TTS audio in chunks"""
        import subprocess
        import tempfile
        import shutil

        try:
            piper_path = shutil.which("piper") or os.path.expanduser("~/.local/bin/piper")
            voice_dir = os.path.expanduser("~/.local/share/piper")

            # Find voice model
            voice_model = None
            for f in os.listdir(voice_dir) if os.path.exists(voice_dir) else []:
                if f.endswith('.onnx') and not f.endswith('.json'):
                    voice_model = os.path.join(voice_dir, f)
                    break

            if not voice_model:
                return

            # Generate full audio first (Piper doesn't support streaming)
            with tempfile.NamedTemporaryFile(mode='w', suffix='.txt', delete=False) as f:
                f.write(text)
                text_path = f.name

            with tempfile.NamedTemporaryFile(suffix='.wav', delete=False) as f:
                wav_path = f.name

            subprocess.run([
                piper_path, '--model', voice_model,
                '--input_file', text_path,
                '--output_file', wav_path
            ], capture_output=True)

            os.unlink(text_path)

            # Read and chunk the audio
            with open(wav_path, 'rb') as f:
                audio_data = f.read()

            os.unlink(wav_path)

            # Skip WAV header (44 bytes) and send in chunks
            audio_data = audio_data[44:]
            chunk_size = 3200  # 100ms at 16kHz, 16-bit mono

            for i in range(0, len(audio_data), chunk_size):
                if self.should_interrupt:
                    break
                chunk = audio_data[i:i+chunk_size]
                await self.tts_audio_queue.put(chunk)
                await asyncio.sleep(0.05)  # Simulate streaming

        except Exception as e:
            logger.error(f"TTS generation error: {e}")


# Connected sessions
sessions = {}


async def streaming_websocket_handler(request):
    """Handle streaming WebSocket connections"""
    ws = web.WebSocketResponse()
    await ws.prepare(request)

    client_id = id(ws)
    logger.info(f"Client {client_id} connected")

    session = StreamingVoiceSession(ws)
    sessions[client_id] = session

    try:
        await session.send_status("connected", "Connected. Tap mic to speak.")

        async for msg in ws:
            if msg.type == aiohttp.WSMsgType.BINARY:
                # Received audio chunk
                audio_data = np.frombuffer(msg.data, dtype=np.float32)
                await session.process_streaming_audio(audio_data)

            elif msg.type == aiohttp.WSMsgType.TEXT:
                data = json.loads(msg.data)
                msg_type = data.get("type")

                if msg_type == "start":
                    # Client started streaming
                    session.is_listening = True
                    await session.send_status("ready", "Listening...")

                elif msg_type == "stop":
                    # Client stopped streaming
                    session.is_listening = False
                    if session.current_utterance:
                        await session.process_utterance()

                elif msg_type == "interrupt":
                    # Manual interrupt
                    await session.interrupt_speaking()

                elif msg_type == "ping":
                    await ws.send_json({"type": "pong"})

            elif msg.type == aiohttp.WSMsgType.ERROR:
                logger.error(f"WebSocket error: {ws.exception()}")

    except Exception as e:
        logger.error(f"Session error: {e}")
    finally:
        del sessions[client_id]
        logger.info(f"Client {client_id} disconnected")

    return ws


async def index_handler(request):
    """Serve the main HTML file"""
    try:
        html_content = open(os.path.join(os.path.dirname(__file__), 'streaming-voice.html')).read()
        return web.Response(text=html_content, content_type='text/html')
    except:
        return web.Response(text="HTML file not found", status=404)


async def main():
    import argparse

    parser = argparse.ArgumentParser(description='Streaming Voice Assistant Server')
    parser.add_argument('--port', type=int, default=8766, help='Port to listen on')
    args = parser.parse_args()

    app = web.Application()
    app.router.add_get('/', index_handler)
    app.router.add_get('/ws', streaming_websocket_handler)
    app.router.add_static('/', path=os.path.dirname(__file__), show_index=False)

    runner = web.AppRunner(app)
    await runner.setup()

    site = web.TCPSite(runner, '0.0.0.0', args.port)
    await site.start()

    logger.info(f"=" * 60)
    logger.info(f"Streaming Voice Server on http://0.0.0.0:{args.port}")
    logger.info(f"Features: Interruption, Backchanneling, Streaming")
    logger.info(f"=" * 60)

    while True:
        await asyncio.sleep(3600)


if __name__ == "__main__":
    asyncio.run(main())

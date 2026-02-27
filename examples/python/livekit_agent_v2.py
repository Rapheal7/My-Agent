#!/usr/bin/env python3
"""
LiveKit Voice Agent V2 - Improved Reliability
A fully local voice assistant using free AI models.

Stack:
- Faster-Whisper for STT (local, free)
- Grok 4.1 Fast via OpenRouter for LLM (free tier)
- Piper for TTS (local, free)
- Silero for VAD (local, free)

Run with: python3 livekit_agent_v2.py
"""

import asyncio
import json
import logging
import os
import tempfile
import wave
import time
from pathlib import Path
from typing import Optional

# Configure logging
logging.basicConfig(
    level=logging.INFO,
    format='%(asctime)s - %(name)s - %(levelname)s - %(message)s'
)
logger = logging.getLogger(__name__)

# Load environment variables
def load_env():
    env_file = Path(__file__).parent / ".env"
    if env_file.exists():
        with open(env_file) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith('#') and '=' in line:
                    key, value = line.split('=', 1)
                    os.environ[key] = value

load_env()

from livekit import rtc
from livekit.agents import (
    Agent, AgentSession, AutoSubscribe,
    JobContext, WorkerOptions, cli,
    llm, stt, tts, vad
)
from livekit.agents.types import DEFAULT_API_CONNECT_OPTIONS
from livekit.agents.tts import AudioEmitter
from livekit.plugins import openai, silero
from livekit.api import AccessToken, VideoGrants

# Constants
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"
GROK_MODEL = "x-ai/grok-4.1-fast"
SYSTEM_PROMPT = """You are a helpful voice assistant. Keep responses concise and conversational.
Be friendly and helpful. Respond naturally as if speaking to a friend.
Keep responses brief - 1-3 sentences when possible."""


class FasterWhisperSTT(stt.STT):
    """Local Faster-Whisper STT - runs entirely on your machine."""

    def __init__(self, model_size: str = "base"):
        super().__init__(capabilities=stt.STTCapabilities(streaming=False, interim_results=False))
        self.model_size = model_size
        self._model = None
        logger.info(f"FasterWhisperSTT initialized with model: {model_size}")

    def _get_model(self):
        if self._model is None:
            from faster_whisper import WhisperModel
            logger.info(f"Loading Faster-Whisper model: {self.model_size}")
            self._model = WhisperModel(self.model_size, device="cpu", compute_type="int8")
            logger.info("Faster-Whisper model loaded!")
        return self._model

    async def _recognize_impl(self, buffer, *, language="en", conn_options=None):
        model = self._get_model()

        if isinstance(buffer, list):
            all_data = b"".join(frame.data for frame in buffer)
            sample_rate = buffer[0].sample_rate
        else:
            all_data = buffer.data
            sample_rate = buffer.sample_rate

        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            temp_path = f.name

        try:
            with wave.open(temp_path, "wb") as wav_file:
                wav_file.setnchannels(1)
                wav_file.setsampwidth(2)
                wav_file.setframerate(sample_rate)
                wav_file.writeframes(all_data)

            loop = asyncio.get_running_loop()
            segments, info = await loop.run_in_executor(
                None, lambda: model.transcribe(temp_path, language=language)
            )

            text = "".join(segment.text for segment in segments).strip()
            logger.info(f"STT: '{text}'")

            return stt.SpeechEvent(
                type=stt.SpeechEventType.FINAL_TRANSCRIPT,
                alternatives=[stt.SpeechData(language=language, text=text, confidence=1.0)],
            )
        finally:
            try:
                os.unlink(temp_path)
            except Exception:
                pass


class PiperChunkedStream(tts.ChunkedStream):
    """Custom ChunkedStream for Piper TTS."""

    def __init__(self, tts_instance: "PiperTTS", input_text: str, conn_options):
        super().__init__(tts=tts_instance, input_text=input_text, conn_options=conn_options)
        self._piper_tts = tts_instance
        self._input_text = input_text

    async def _run(self, output_emitter: AudioEmitter) -> None:
        synthesizer = self._piper_tts._get_synthesizer()
        output_emitter.initialize(sample_rate=self._piper_tts.SAMPLE_RATE)

        import io
        audio_buffer = io.BytesIO()

        for audio_chunk in synthesizer.synthesize_stream_raw(self._input_text):
            audio_buffer.write(audio_chunk)

        audio_bytes = audio_buffer.getvalue()

        if audio_bytes:
            logger.info(f"TTS: Generated {len(audio_bytes)} bytes")
            output_emitter.push(audio_bytes)
        else:
            logger.warning("TTS: No audio generated")

        output_emitter.end_input()
        output_emitter.flush()


class PiperTTS(tts.TTS):
    """Local Piper TTS - runs entirely on your machine."""

    SAMPLE_RATE = 22050

    def __init__(self, voice: str = "en_US-lessac-medium"):
        super().__init__(
            capabilities=tts.TTSCapabilities(streaming=False),
            sample_rate=self.SAMPLE_RATE,
            num_channels=1,
        )
        self.voice = voice
        self._synthesizer = None
        logger.info(f"PiperTTS initialized with voice: {voice}")

    def _get_synthesizer(self):
        if self._synthesizer is None:
            from piper import PiperVoice
            logger.info(f"Loading Piper voice: {self.voice}")
            self._synthesizer = PiperVoice.load(self.voice)
            logger.info("Piper voice loaded!")
        return self._synthesizer

    def synthesize(self, text: str, *, conn_options=DEFAULT_API_CONNECT_OPTIONS):
        return PiperChunkedStream(self, text, conn_options)


async def run_explicit_room_mode(room_name: str):
    """Run agent and join a specific room directly with token refresh."""
    logger.info(f"Starting agent in explicit room mode: {room_name}")

    api_key = os.getenv("LIVEKIT_API_KEY")
    api_secret = os.getenv("LIVEKIT_API_SECRET")
    livekit_url = os.getenv("LIVEKIT_URL", "wss://my-agent-t6shkefq.livekit.cloud")
    openrouter_key = os.getenv("OPENROUTER_API_KEY")

    if not api_key or not api_secret:
        logger.error("LIVEKIT_API_KEY and LIVEKIT_API_SECRET must be set")
        return

    if not openrouter_key:
        logger.error("OPENROUTER_API_KEY must be set")
        return

    # Initialize components
    logger.info("Loading STT...")
    stt_instance = FasterWhisperSTT(model_size="base")

    logger.info("Loading LLM...")
    llm_instance = openai.LLM(
        model=GROK_MODEL,
        base_url=OPENROUTER_BASE_URL,
        api_key=openrouter_key,
        extra_headers={
            "HTTP-Referer": "https://github.com/my-agent",
            "X-Title": "My Agent Voice Assistant",
        },
    )

    logger.info("Loading TTS...")
    tts_instance = PiperTTS(voice="en_US-lessac-medium")

    logger.info("Loading VAD...")
    vad_instance = silero.VAD.load()

    # Keep running with reconnection logic
    while True:
        try:
            # Generate fresh token
            token = AccessToken(api_key, api_secret) \
                .with_identity("voice-agent") \
                .with_name("Voice Agent") \
                .with_grants(VideoGrants(
                    room_join=True,
                    room=room_name,
                    can_publish=True,
                    can_subscribe=True,
                    can_publish_data=True,
                ))
            jwt_token = token.to_jwt()
            logger.info(f"Generated fresh token for room '{room_name}'")

            session = AgentSession(stt=stt_instance, llm=llm_instance, tts=tts_instance, vad=vad_instance)
            agent = Agent(instructions=SYSTEM_PROMPT)

            # Set up event handlers
            @session.on("user_speech_committed")
            def on_user_speech(event):
                logger.info(f"User: {event}")

            @session.on("agent_speech_committed")
            def on_agent_speech(event):
                logger.info(f"Agent: {event}")

            @session.on("user_started_speaking")
            def on_user_started():
                logger.info("User started speaking")

            @session.on("user_stopped_speaking")
            def on_user_stopped():
                logger.info("User stopped speaking - processing...")

            # Connect to room
            room = rtc.Room()

            @room.on("participant_connected")
            def on_participant(participant):
                logger.info(f"Participant joined: {participant.identity}")

            @room.on("participant_disconnected")
            def on_participant_left(participant):
                logger.info(f"Participant left: {participant.identity}")

            @room.on("track_subscribed")
            def on_track_subscribed(track, publication, participant):
                logger.info(f"Track subscribed: {track.kind} from {participant.identity}")

            @room.on("disconnected")
            def on_disconnected():
                logger.info("Room disconnected")

            await room.connect(livekit_url, jwt_token, options=rtc.RoomOptions(auto_subscribe=True))
            logger.info(f"Connected to room: {room.name}")

            # Start session
            await session.start(agent=agent, room=room)
            logger.info("Voice agent is READY!")

            # Keep running until disconnected
            while room.connection_state == rtc.ConnectionState.CONN_CONNECTED:
                await asyncio.sleep(1)

            logger.info("Connection lost, will reconnect in 2 seconds...")
            await asyncio.sleep(2)

        except Exception as e:
            logger.error(f"Error: {e}")
            logger.info("Reconnecting in 5 seconds...")
            await asyncio.sleep(5)


if __name__ == "__main__":
    import sys

    room_name = "voice-session"

    # Allow room override
    if "--room" in sys.argv:
        idx = sys.argv.index("--room")
        if idx + 1 < len(sys.argv):
            room_name = sys.argv[idx + 1]

    logger.info("=" * 50)
    logger.info("LiveKit Voice Agent V2")
    logger.info("=" * 50)
    logger.info("Stack:")
    logger.info("  STT: Faster-Whisper (local, free)")
    logger.info("  LLM: Grok 4.1 Fast (OpenRouter, free)")
    logger.info("  TTS: Piper (local, free)")
    logger.info("  VAD: Silero (local, free)")
    logger.info("=" * 50)
    logger.info(f"Room: {room_name}")
    logger.info("=" * 50)

    asyncio.run(run_explicit_room_mode(room_name))

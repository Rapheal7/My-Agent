"""
LiveKit Voice Agent with Local AI Components

A fully local voice assistant using:
- Faster-Whisper for speech-to-text (local, free, fast)
- OpenRouter/Grok 4.1 Fast for LLM (free, smart)
- Piper for text-to-speech (local, free)
- Silero for voice activity detection

Run with: python livekit_agent.py
"""

import asyncio
import json
import logging
import os
import tempfile
import wave
from typing import AsyncIterator
from pathlib import Path

# Configure logging first
logging.basicConfig(level=logging.INFO)
logger = logging.getLogger(__name__)

# Load environment variables from .env file if it exists
def load_env():
    env_file = Path(__file__).parent / ".env"
    if env_file.exists():
        logger.info(f"Loading environment from {env_file}")
        with open(env_file) as f:
            for line in f:
                line = line.strip()
                if line and not line.startswith('#') and '=' in line:
                    key, value = line.split('=', 1)
                    os.environ[key] = value
                    logger.info(f"Loaded env var: {key}")

load_env()

from livekit import rtc
from livekit.agents import (
    Agent,
    AgentSession,
    AutoSubscribe,
    JobContext,
    WorkerOptions,
    cli,
    llm,
    stt,
    tts,
    vad,
)
from livekit.agents.types import DEFAULT_API_CONNECT_OPTIONS, NOT_GIVEN
from livekit.agents.tts import AudioEmitter
from livekit.plugins import openai, silero

# Constants
OPENROUTER_BASE_URL = "https://openrouter.ai/api/v1"
GROK_MODEL = "x-ai/grok-4.1-fast"
SYSTEM_PROMPT = """You are a helpful voice assistant. Keep responses concise and conversational.
Be friendly and helpful. Respond naturally as if speaking to a friend."""


class FasterWhisperSTT(stt.STT):
    """
    Local Faster-Whisper STT - runs entirely on your machine!
    Uses CTranslate2 for optimized inference.
    """

    def __init__(self, model_size: str = "base"):
        """Initialize Faster-Whisper STT.

        Args:
            model_size: Whisper model size (tiny, base, small, medium, large-v3)
        """
        super().__init__(capabilities=stt.STTCapabilities(streaming=False, interim_results=False))
        self.model_size = model_size
        self._model = None
        logger.info(f"FasterWhisperSTT initialized with model: {model_size}")

    def _get_model(self):
        """Lazy load the Faster-Whisper model."""
        if self._model is None:
            from faster_whisper import WhisperModel
            logger.info(f"Loading Faster-Whisper model: {self.model_size}")
            self._model = WhisperModel(
                self.model_size, device="cpu", compute_type="int8"
            )
            logger.info("Faster-Whisper model loaded!")
        return self._model

    async def _recognize_impl(
        self,
        buffer: rtc.AudioFrame | list[rtc.AudioFrame],
        *,
        language: str = "en",
        conn_options,
    ) -> stt.SpeechEvent:
        """Transcribe audio data to text using Faster-Whisper.

        Args:
            buffer: Audio frame(s) to transcribe
            language: Language code for transcription

        Returns:
            SpeechEvent with transcription
        """
        model = self._get_model()

        # Handle single frame or list of frames
        if isinstance(buffer, list):
            # Concatenate all frames
            all_data = b"".join(frame.data for frame in buffer)
            sample_rate = buffer[0].sample_rate
        else:
            all_data = buffer.data
            sample_rate = buffer.sample_rate

        # Save to temporary WAV file (Faster-Whisper expects a file)
        with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
            temp_path = f.name

        try:
            # Write WAV file
            with wave.open(temp_path, "wb") as wav_file:
                wav_file.setnchannels(1)
                wav_file.setsampwidth(2)  # 16-bit
                wav_file.setframerate(sample_rate)
                wav_file.writeframes(all_data)

            # Transcribe with Faster-Whisper
            loop = asyncio.get_running_loop()
            segments, info = await loop.run_in_executor(
                None, lambda: model.transcribe(temp_path, language=language)
            )

            # Combine all segments into final text
            text = "".join(segment.text for segment in segments).strip()
            logger.info("=" * 60)
            logger.info("👂 STT HEARD (Faster-Whisper transcription):")
            logger.info(f"   Text: '{text}'")
            logger.info(f"   Language: {language}")
            logger.info(f"   Duration: {info.duration:.2f}s")
            logger.info("=" * 60)

            return stt.SpeechEvent(
                type=stt.SpeechEventType.FINAL_TRANSCRIPT,
                alternatives=[
                    stt.SpeechData(
                        language=language,
                        text=text,
                        confidence=1.0,
                    )
                ],
            )
        finally:
            # Clean up temp file
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
        """Generate audio and push to output emitter."""
        logger.info("=" * 60)
        logger.info("🗣️ TTS GENERATING SPEECH (Piper)")
        logger.info(f"   Input text: '{self._input_text[:100]}{'...' if len(self._input_text) > 100 else ''}'")
        logger.info("=" * 60)

        synthesizer = self._piper_tts._get_synthesizer()

        # Initialize output with 48000 Hz for LiveKit compatibility
        output_emitter.initialize(sample_rate=48000)
        logger.info(f"   Initialized emitter with sample_rate=48000")

        # Generate audio using Piper's stream synthesis
        import io
        audio_buffer = io.BytesIO()

        # Collect all audio chunks
        for audio_chunk in synthesizer.synthesize_stream_raw(self._input_text):
            audio_buffer.write(audio_chunk)

        audio_bytes = audio_buffer.getvalue()

        if audio_bytes:
            # Push the raw PCM audio data
            # Piper outputs 16-bit signed PCM at 22050Hz
            logger.info("=" * 60)
            logger.info("🔊 TTS AUDIO READY (Piper)")
            logger.info(f"   Generated: {len(audio_bytes)} bytes")
            logger.info(f"   Sample rate: {self._piper_tts.SAMPLE_RATE} Hz")
            logger.info(f"   Duration: ~{len(audio_bytes) / (self._piper_tts.SAMPLE_RATE * 2):.2f}s")
            logger.info("=" * 60)
            logger.info("Pushing audio to emitter...")
            output_emitter.push(audio_bytes)
            logger.info("Audio pushed to emitter!")
        else:
            logger.warning("=" * 60)
            logger.warning("⚠️ TTS GENERATED NO AUDIO DATA")
            logger.warning("=" * 60)

        # Signal end of input and flush
        output_emitter.end_input()
        output_emitter.flush()

        logger.info("=" * 60)
        logger.info("✅ TTS COMPLETE - Audio sent to output emitter")
        logger.info(f"   Text: '{self._input_text[:50]}{'...' if len(self._input_text) > 50 else ''}'")
        logger.info("=" * 60)


class PiperTTS(tts.TTS):
    """
    Local Piper TTS - runs entirely on your machine!
    Fast, natural-sounding voices.
    """

    SAMPLE_RATE = 22050

    def __init__(self, voice: str = "en_US-lessac-medium"):
        """Initialize Piper TTS.

        Args:
            voice: Voice name (e.g., en_US-lessac-medium)
        """
        super().__init__(
            capabilities=tts.TTSCapabilities(streaming=False),
            sample_rate=self.SAMPLE_RATE,
            num_channels=1,
        )
        self.voice = voice
        self._synthesizer = None
        logger.info(f"PiperTTS initialized with voice: {voice}")

    def _get_synthesizer(self):
        """Lazy load the Piper synthesizer."""
        if self._synthesizer is None:
            from piper import PiperVoice
            logger.info(f"Loading Piper voice: {self.voice}")
            self._synthesizer = PiperVoice.load(self.voice)
            logger.info("Piper voice loaded!")
        return self._synthesizer

    def synthesize(
        self, text: str, *, conn_options=DEFAULT_API_CONNECT_OPTIONS
    ) -> tts.ChunkedStream:
        """Synthesize text to audio using Piper.

        Args:
            text: Text to synthesize

        Returns:
            ChunkedStream yielding audio chunks
        """
        return PiperChunkedStream(self, text, conn_options)


async def entrypoint(ctx: JobContext):
    """
    Main entrypoint for the LiveKit voice agent.

    This creates a multimodal agent with:
    - Faster-Whisper for speech-to-text (local, free)
    - OpenRouter/Grok for the brain (free, smart)
    - Piper for text-to-speech (local, free)
    - Silero for voice activity detection
    """
    logger.info(f"Starting voice agent for room: {ctx.room.name}")

    # Get API keys from environment
    openrouter_key = os.getenv("OPENROUTER_API_KEY")

    if not openrouter_key:
        logger.error("OPENROUTER_API_KEY not set!")
        raise ValueError("OPENROUTER_API_KEY is required for LLM")

    # Initialize the agent components
    # STT: Faster-Whisper (local, free)
    logger.info("Initializing Faster-Whisper STT...")
    stt_instance = FasterWhisperSTT(model_size="base")

    # LLM: Grok 4.1 Fast via OpenRouter (using OpenAI-compatible API)
    logger.info("Initializing Grok LLM via OpenRouter...")
    llm_instance = openai.LLM(
        model=GROK_MODEL,
        base_url=OPENROUTER_BASE_URL,
        api_key=openrouter_key,
        extra_headers={
            "HTTP-Referer": "https://github.com/my-agent",
            "X-Title": "My Agent Voice Assistant",
        },
    )

    # TTS: Piper (local, free)
    logger.info("Initializing Piper TTS...")
    tts_instance = PiperTTS(voice="en_US-lessac-medium")

    # VAD: Silero for voice activity detection
    logger.info("Loading Silero VAD...")
    vad_instance = silero.VAD.load()

    # Create the session - NOTE: Disabling VAD to use simpler speech detection
    logger.info("Creating AgentSession without VAD (using STT for speech detection)...")
    session = AgentSession(
        stt=stt_instance,
        llm=llm_instance,
        tts=tts_instance,
        # No VAD - let STT detect speech
    )

    # Create the agent with instructions
    agent = Agent(
        instructions=SYSTEM_PROMPT,
    )

    # Connect to the LiveKit room
    logger.info("Connecting to LiveKit room...")
    await ctx.connect(auto_subscribe=AutoSubscribe.AUDIO_ONLY)

    # Start the agent
    logger.info("Starting voice agent session...")
    await session.start(agent=agent, room=ctx.room)

    logger.info("=" * 50)
    logger.info("Voice agent is READY!")
    logger.info(f"Room: {ctx.room.name}")
    logger.info("STT: Faster-Whisper (local, CTranslate2)")
    logger.info("LLM: Grok 4.1 Fast (OpenRouter)")
    logger.info("TTS: Piper (local)")
    logger.info("=" * 50)


if __name__ == "__main__":
    import sys

    # Check for explicit room mode
    # Usage: python livekit_agent.py --room <room_name>
    if "--room" in sys.argv:
        room_idx = sys.argv.index("--room")
        if room_idx + 1 < len(sys.argv):
            explicit_room = sys.argv[room_idx + 1]
        else:
            print("Error: --room requires a room name")
            sys.exit(1)

        # Explicit room mode - join the specified room directly
        logger.info(f"Explicit room mode: joining room '{explicit_room}'")

        async def explicit_entrypoint():
            """Run the agent and join a specific room."""
            # Get API credentials
            api_key = os.getenv("LIVEKIT_API_KEY")
            api_secret = os.getenv("LIVEKIT_API_SECRET")
            livekit_url = os.getenv("LIVEKIT_URL", "wss://my-agent-t6shkefq.livekit.cloud")

            if not api_key or not api_secret:
                logger.error("LIVEKIT_API_KEY and LIVEKIT_API_SECRET must be set")
                return

            # Create access token for the agent using official LiveKit API
            from livekit.api import AccessToken, VideoGrants

            token = AccessToken(api_key, api_secret) \
                .with_identity("voice-agent") \
                .with_name("Voice Agent") \
                .with_grants(VideoGrants(
                    room_join=True,
                    room=explicit_room,
                    can_publish=True,
                    can_subscribe=True,
                    can_publish_data=True,
                ))

            jwt_token = token.to_jwt()
            logger.info(f"Generated token for room '{explicit_room}' (first 50 chars): {jwt_token[:50]}...")

            # Initialize components
            openrouter_key = os.getenv("OPENROUTER_API_KEY")
            if not openrouter_key:
                logger.error("OPENROUTER_API_KEY not set!")
                return

            logger.info("Loading STT (Faster-Whisper)...")
            stt_instance = FasterWhisperSTT(model_size="base")

            logger.info("Loading LLM (Grok via OpenRouter)...")
            llm_instance = openai.LLM(
                model=GROK_MODEL,
                base_url=OPENROUTER_BASE_URL,
                api_key=openrouter_key,
                extra_headers={
                    "HTTP-Referer": "https://github.com/my-agent",
                    "X-Title": "My Agent Voice Assistant",
                },
            )

            logger.info("Loading TTS (Piper)...")
            tts_instance = PiperTTS(voice="en_US-lessac-medium")

            logger.info("Loading VAD (Silero)...")
            vad_instance = silero.VAD.load()

            from livekit.agents import (
                AutoSubscribe,
                RoomInputOptions,
                RoomOutputOptions,
            )

            session = AgentSession(
                stt=stt_instance,
                llm=llm_instance,
                tts=tts_instance,
                vad=vad_instance,
            )

            # Add event handlers for debugging
            @session.on("user_speech_committed")
            def on_user_speech(event):
                logger.info("=" * 60)
                logger.info("👂 USER SPEECH DETECTED (HEARING)")
                logger.info(f"   Event type: user_speech_committed")
                logger.info(f"   Event data: {event}")
                logger.info("=" * 60)

            @session.on("agent_speech_committed")
            def on_agent_speech(event):
                logger.info("=" * 60)
                logger.info("🗣️ AGENT SPEECH COMMITTED (SPEAKING BACK)")
                logger.info(f"   Event type: agent_speech_committed")
                logger.info(f"   Event data: {event}")
                logger.info("=" * 60)

            @session.on("speech_created")
            def on_speech_created(event):
                logger.info("=" * 60)
                logger.info("📝 SPEECH CREATED")
                logger.info(f"   Event type: speech_created")
                logger.info(f"   Event data: {event}")
                logger.info("=" * 60)

            @session.on("user_started_speaking")
            def on_user_started_speaking():
                logger.info("=" * 60)
                logger.info("🎤 USER STARTED SPEAKING")
                logger.info("   VAD detected voice activity")
                logger.info("=" * 60)
                # Send to browser via data channel
                try:
                    asyncio.create_task(room.local_participant.publish_data(
                        json.dumps({"status": "listening", "message": "Hearing you..."}).encode()
                    ))
                except Exception as e:
                    logger.warning(f"Failed to send data: {e}")

            @session.on("user_stopped_speaking")
            def on_user_stopped_speaking():
                logger.info("=" * 60)
                logger.info("🔇 USER STOPPED SPEAKING")
                logger.info("   Processing speech...")
                logger.info("=" * 60)

            # Additional event handlers for more visibility
            @session.on("agent_started_speaking")
            def on_agent_started_speaking():
                logger.info("=" * 60)
                logger.info("🔊 AGENT STARTED SPEAKING (AUDIO OUTPUT)")
                logger.info("   TTS is generating audio response")
                logger.info("=" * 60)
                # Notify browser
                try:
                    asyncio.create_task(room.local_participant.publish_data(
                        json.dumps({"status": "speaking", "message": "Agent is responding..."}).encode()
                    ))
                except Exception as e:
                    logger.warning(f"Failed to send data: {e}")

            @session.on("agent_stopped_speaking")
            def on_agent_stopped_speaking():
                logger.info("=" * 60)
                logger.info("🔇 AGENT STOPPED SPEAKING")
                logger.info("   Audio response complete")
                logger.info("=" * 60)

            agent = Agent(instructions=SYSTEM_PROMPT)

            # Connect to room directly with auto_subscribe for audio
            from livekit import rtc

            logger.info(f"Connecting to room: {explicit_room}")
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
                if track.kind == 'audio':
                    logger.info("🎤 AUDIO TRACK RECEIVED FROM USER!")
                    logger.info(f"   Track info: {track}")
                    # Log audio track details
                    try:
                        if hasattr(track, 'mediaStreamTrack'):
                            logger.info(f"   MediaStreamTrack: {track.mediaStreamTrack}")
                    except Exception as e:
                        logger.warning(f"Could not get track info: {e}")

            @room.on("track_unsubscribed")
            def on_track_unsubscribed(track, publication, participant):
                logger.info(f"Track unsubscribed: {track.kind} from {participant.identity}")

            # Connect with auto_subscribe enabled
            await room.connect(livekit_url, jwt_token, options=rtc.RoomOptions(auto_subscribe=True))
            logger.info(f"Connected to room: {room.name}")

            # Start the agent session with proper options
            await session.start(
                agent=agent,
                room=room,
                room_input_options=RoomInputOptions(
                    audio_enabled=True,
                ),
                room_output_options=RoomOutputOptions(
                    audio_enabled=True,
                    audio_sample_rate=48000,  # Standard WebRTC sample rate
                ),
            )
            logger.info("=" * 50)
            logger.info("Voice agent is READY!")
            logger.info(f"Room: {room.name}")
            logger.info("=" * 50)

            # Send greeting to confirm audio is working
            # Note: agent.say() is not available in this version
            # The session handles automatic responses when user speaks
            logger.info("Agent ready! Waiting for user to speak...")

            # Keep running
            while True:
                await asyncio.sleep(1)

        asyncio.run(explicit_entrypoint())
    else:
        # Normal worker mode - wait for dispatch from LiveKit Cloud
        cli.run_app(
            WorkerOptions(
                entrypoint_fnc=entrypoint,
                agent_name="my-agent-voice",
            )
        )

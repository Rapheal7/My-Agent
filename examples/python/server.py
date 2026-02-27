# server.py
import asyncio
import os
from fastapi import FastAPI, Request
from fastapi.staticfiles import StaticFiles
from fastapi.templating_jinja2 import Jinja2Templates
from aiortc import RTCPeerConnection, RTCSessionDescription, MediaStreamTrack
from typing import Dict

# Serve static HTML
app = FastAPI()
app.mount("/static", StaticFiles(directory="static"), name="static")
templates = Jinja2Templates(directory="templates")

# ---- 1️⃣ Load quantized Sesame model ----------------------------------------------------
# Adjust path if needed
MODEL_PATH = os.getenv("SESAME_MODEL_PATH", "./models/sesame/Sesame-13B-4bit-128g-GGUF")
# Import Llama from llama_cpp (GGUF support)
from llama_cpp import Llama
# Create a singleton model instance (runs synchronously; we’ll offload to thread pool)
llm = Llama(
    model_path=MODEL_PATH,
    n_gpu_layers=0,          # Change to 30+ if you have GPU offload
    n_ctx=2048,
    verbose=False,
)

# Simple sync function that returns generated text
def generate_llm_text(prompt: str) -> str:
    # Use the LLM directly; you can tweak parameters
    output = llm(
        prompt,
        max_tokens=256,
        temperature=0.7,
        top_p=0.9,
        repeat_penalty=1.1,
        stop=["\n\n"],
    )
    # The output is a dict; the generated text is in output['choices'][0]['text']
    return output["choices"][0]["text"].strip()

# async wrapper for integration with aiortc
async def run_generate(prompt: str) -> str:
    loop = asyncio.get_running_loop()
    # Offload to thread pool to avoid blocking
    return await loop.run_in_executor(None, generate_llm_text, prompt)

# ---- 2️⃣ TTS (placeholder – replace with actual Sesame TTS) ---------------------------
async def async_speak(text: str) -> bytes:
    # Replace with real TTS call (e.g., Sesame, Coqui, etc.)
    # For demo, return simple dummy audio bytes
    return b"dummy-audio-data"

# ---- 3️⃣ Audio reply track -----------------------------------------------------------
class AudioReplyTrack(MediaStreamTrack):
    kind = "audio"

    def __init__(self, queue: asyncio.Queue):
        super().__init__()
        self.queue = queue

    async def recv(self) -> bytes:
        data = await self.queue.get()
        return await self.speak(data)

    async def speak(self, data: bytes) -> bytes:
        # Decode text, generate reply, synthesize speech
        text = data.decode("utf-8")
        reply = await run_generate(text)
        return await async_speak(reply)

# ---- 4️⃣ Serve the client UI ---------------------------------------------------------
@app.get("/")
async def index(request: Request):
    return templates.TemplateResponse("index.html", {})

# ---- 5️⃣ WebRTC signalling -----------------------------------------------------------
@app.post("/webrtc")
async def webrtc(request: Request):
    params = await request.json()
    offer = RTCSessionDescription(sdp=params["sdp"], type=params["type"])

    pc = RTCPeerConnection()
    # Tailscale public ICE server – works out‑of‑the‑box
    pc.configureIceServers([{"urls": ["wss://random-ice.tailscale.com"]}])

    # Create queue for audio data coming back from the server
    reply_queue = asyncio.Queue()
    reply_track = AudioReplyTrack(reply_queue)
    pc.addTrack(reply_track)

    @pc.on("track")
    def on_track(track):
        if track.kind == "audio":
            # Forward incoming audio (microphone) to our processing pipeline
            asyncio.create_task(handle_incoming_audio(track, reply_queue))

    # Set remote description (client’s offer) and craft answer
    await pc.setRemoteDescription(offer)
    answer = await pc.createAnswer()
    await pc.setLocalDescription(answer)

    return {
        "sdp": pc.localDescription.sdp,
        "type": pc.localDescription.type,
    }

# ---- 6️⃣ Process incoming audio (microphone) -----------------------------------------
async def handle_incoming_audio(track: MediaStreamTrack, reply_queue: asyncio.Queue):
    """Pull audio frames, run ASR → LLM → TTS, then enqueue TTS bytes."""
    async for frame in track.audio:
        # Convert frame to string (dummy conversion for illustration)
        pcm_str = frame.to_ndarray().tobytes().decode("latin1")
        # TODO: Replace with real ASR (e.g., Whisper) to get spoken text
        spoken_text = pcm_str  # placeholder
        # Generate LLM reply
        reply_text = await run_generate(spoken_text)
        # Synthesize speech
        audio_bytes = await async_speak(reply_text)
        await reply_queue.put(audio_bytes)

# ---- 7️⃣ Run -----------------------------------------------------------------------
if __name__ == "__main__":
    import uvicorn
    # 0.0.0.0 makes the server reachable on the Tailscale interface
    uvicorn.run(app, host="0.0.0.0", port=8000)
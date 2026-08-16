import os
import tempfile
import asyncio
from typing import Optional
from fastapi import FastAPI, BackgroundTasks, HTTPException, Header, Query
from pydantic import BaseModel
import httpx
import yt_dlp

app = FastAPI(title="BeatVault yt-dlp Ingestion Service")

GATEWAY_URL = os.getenv("GATEWAY_URL", "http://gateway.bvault-prod.svc.cluster.local:8080")
INTERNAL_API_KEY = os.getenv("INTERNAL_API_KEY", "")

class ExtractRequest(BaseModel):
    url: str
    user_id: str
    gateway_url: Optional[str] = None

from yt_dlp.networking.impersonate import ImpersonateTarget

def process_yt_dlp(url: str, user_id: str, target_gateway_url: str):
    ydl_opts = {
        'format': 'bestaudio/best',
        'outtmpl': os.path.join(tempfile.gettempdir(), '%(id)s.%(ext)s'),
        'postprocessors': [{
            'key': 'FFmpegExtractAudio',
            'preferredcodec': 'mp3',
            'preferredquality': '320',
        }],
        'quiet': True,
        'no_warnings': True,
        'impersonate': ImpersonateTarget.from_str('chrome'),
        'extractor_args': {'youtube': ['player_client=web']},
    }

    with yt_dlp.YoutubeDL(ydl_opts) as ydl:
        info = ydl.extract_info(url, download=True)
        filename = ydl.prepare_filename(info)
        # Postprocessor converts to .mp3
        mp3_filename = os.path.splitext(filename)[0] + ".mp3"
        title = info.get("title") or info.get("track") or "Unknown Title"
        artist = info.get("artist") or info.get("uploader") or "Unknown Artist"

        if os.path.exists(mp3_filename):
            with open(mp3_filename, "rb") as f:
                audio_bytes = f.read()

            try:
                files = {
                    "file": (os.path.basename(mp3_filename), audio_bytes, "audio/mpeg"),
                    "title": (None, title),
                    "artist": (None, artist),
                }
                headers = {"X-User-Id": user_id}
                if INTERNAL_API_KEY:
                    headers["X-Internal-Key"] = INTERNAL_API_KEY

                upload_url = f"{target_gateway_url.rstrip('/')}/ingest/upload"
                print(f"[yt-dlp-ingest] Uploading '{title}' to gateway...", flush=True)
                res = httpx.post(upload_url, files=files, headers=headers, timeout=60.0)
                print(f"[yt-dlp-ingest] Ingested '{title}' status: {res.status_code}", flush=True)
            finally:
                if os.path.exists(mp3_filename):
                    os.remove(mp3_filename)
        else:
            print(f"[yt-dlp-ingest] Error: MP3 file was not created at {mp3_filename}", flush=True)

@app.get("/health")
def health():
    return {"status": "ok"}

@app.post("/extract")
def extract_audio(req: ExtractRequest, background_tasks: BackgroundTasks):
    target_url = req.gateway_url or GATEWAY_URL
    background_tasks.add_task(process_yt_dlp, req.url, req.user_id, target_url)
    return {"status": "accepted", "message": f"Queued extraction for {req.url}"}

@app.get("/search")
def search_yt_dlp(q: str, limit: int = 10, x_internal_key: Optional[str] = Header(None)):
    if INTERNAL_API_KEY and x_internal_key != INTERNAL_API_KEY:
        raise HTTPException(status_code=401, detail="Invalid internal key")

    ydl_opts = {"extract_flat": True, "quiet": True, "no_warnings": True}
    results = []
    with yt_dlp.YoutubeDL(ydl_opts) as ydl:
        try:
            info = ydl.extract_info(f"ytsearch{limit}:{q}", download=False)
            for entry in info.get("entries", []):
                vid = entry.get("id")
                url = entry.get("url") or entry.get("webpage_url")
                if not url and vid:                        # flat mode may omit URL
                    url = f"https://www.youtube.com/watch?v={vid}"
                results.append({
                    "title": entry.get("title") or "Unknown Title",
                    "url": url or "",
                    "duration_secs": int(entry.get("duration")) if entry.get("duration") is not None else None,
                    "uploader": entry.get("uploader") or "Unknown",
                    "thumbnail": entry.get("thumbnail"),
                    "video_id": vid or "",
                })
        except Exception as e:
            print(f"[yt-dlp-ingest] search error: {e}")
            raise HTTPException(status_code=500, detail="Search failed")
    return results
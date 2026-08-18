import os
import tempfile
import time
import random
import asyncio
from typing import Optional
from fastapi import FastAPI, BackgroundTasks, HTTPException, Header, Query
from pydantic import BaseModel
import httpx
import shutil, uuid
import yt_dlp
from yt_dlp.networking.impersonate import ImpersonateTarget
from anyio import to_thread

app = FastAPI(title="BeatVault yt-dlp Ingestion Service")

@app.on_event("startup")
async def startup_event():
    # Limit concurrent yt-dlp/ffmpeg jobs to 2 per pod to prevent CPU starvation and OOM kills.
    limiter = to_thread.current_default_thread_limiter()
    limiter.total_tokens = 2

    # Pre-initialize yt-dlp to avoid thread race conditions during plugin registration.
    # Without this, multiple threads calling yt_dlp.YoutubeDL() concurrently will crash
    # with "AssertionError: PoTokenProvider already registered".
    try:
        with yt_dlp.YoutubeDL({'quiet': True, 'no_warnings': True}) as ydl:
            pass
    except Exception as e:
        print(f"[startup] yt-dlp pre-init failed: {e}")

    # Start 2 concurrent claim loops to match the thread limiter
    for _ in range(limiter.total_tokens):
        asyncio.create_task(claim_worker_loop())

GATEWAY_URL = os.getenv("GATEWAY_URL", "http://gateway.bvault-prod.svc.cluster.local:8080")
INTERNAL_API_KEY = os.getenv("INTERNAL_API_KEY", "")
POT_PROVIDER_URL = os.getenv("POT_PROVIDER_URL", "http://127.0.0.1:4416")
MAX_RETRIES = int(os.getenv("MAX_RETRIES", "10"))
BASE_BACKOFF_SECONDS = float(os.getenv("BASE_BACKOFF_SECONDS", "10.0"))

async def claim_worker_loop():
    print("[yt-dlp-ingest] worker loop started", flush=True)
    claim_url = f"{GATEWAY_URL.rstrip('/')}/internal/jobs/claim"
    headers = {}
    if INTERNAL_API_KEY:
        headers["X-Internal-Key"] = INTERNAL_API_KEY

    async with httpx.AsyncClient(timeout=30.0) as client:
        while True:
            try:
                res = await client.post(claim_url, json={"kind": "yt_dlp_ingest", "lease_secs": 600}, headers=headers)
                if res.status_code == 204:
                    await asyncio.sleep(5)
                    continue

                res.raise_for_status()
                job = res.json()
                
                url = job["payload"]["url"]
                user_id = job["payload"]["user_id"]
                job_id = job["id"]

                # Process the job via threadpool
                await to_thread.run_sync(process_yt_dlp, url, user_id, job_id, GATEWAY_URL)
                
            except httpx.HTTPError as e:
                print(f"[yt-dlp-ingest] http error claiming job: {e}", flush=True)
                await asyncio.sleep(5)
            except Exception as e:
                print(f"[yt-dlp-ingest] unexpected error in worker loop: {e}", flush=True)
                await asyncio.sleep(5)


def _report_job(job_id: int, user_id: str, gateway_url: str, ok: bool, error: Optional[str] = None):
    """Flip the job's terminal state on the gateway (which owns the DB)."""
    try:
        url = f"{gateway_url.rstrip('/')}/internal/jobs/{job_id}"
        headers = {"X-User-Id": user_id}
        if INTERNAL_API_KEY:
            headers["X-Internal-Key"] = INTERNAL_API_KEY
        httpx.post(url, json={"ok": ok, "error": error}, headers=headers, timeout=30.0)
    except Exception as e:
        print(f"[yt-dlp-ingest] failed to report job {job_id}: {e}", flush=True)


def process_yt_dlp(url: str, user_id: str, job_id: int, target_gateway_url: str):
    for attempt in range(MAX_RETRIES + 1):
        job_tmp = os.path.join(tempfile.gettempdir(), f"ytdlp-{job_id}-{uuid.uuid4().hex[:8]}")
        os.makedirs(job_tmp, exist_ok=True)
        try:
            ydl_opts = {
                'format': 'bestaudio/best',
                'outtmpl': os.path.join(tempfile.gettempdir(), '%(id)s.%(ext)s'),
                'paths': {'home': job_tmp, 'temp': job_tmp},
                'postprocessors': [{
                    'key': 'FFmpegExtractAudio',
                    'preferredcodec': 'mp3',
                    'preferredquality': '320',
                }],
                'quiet': True,
                'no_warnings': True,
                # Library API needs an ImpersonateTarget object, not a bare string
                # (the --impersonate CLI flag does this conversion for you).
                'impersonate': ImpersonateTarget.from_str('chrome'),
                # Fetch a GVS PO token from the bgutil sidecar so YouTube authorizes
                # the media download (this is the fix for the 403). base_url matches
                # the plugin default, set explicitly so it's greppable + overridable.
                'extractor_args': {
                    'youtubepot-bgutilhttp': {'base_url': [POT_PROVIDER_URL]},
                },
            }

            with yt_dlp.YoutubeDL(ydl_opts) as ydl:
                info = ydl.extract_info(url, download=True)
                filename = ydl.prepare_filename(info)
                mp3_filename = os.path.splitext(filename)[0] + ".mp3"
                title = info.get("title") or info.get("track") or "Unknown Title"
                artist = info.get("artist") or info.get("uploader") or "Unknown Artist"

                if not os.path.exists(mp3_filename):
                    raise RuntimeError(f"MP3 file was not created at {mp3_filename}")

                try:
                    with open(mp3_filename, "rb") as f:
                        files = {
                            "file": (os.path.basename(mp3_filename), f, "audio/mpeg"),
                            "title": (None, title),
                            "artist": (None, artist),
                        }
                        headers = {"X-User-Id": user_id}
                        if INTERNAL_API_KEY:
                            headers["X-Internal-Key"] = INTERNAL_API_KEY

                        upload_url = f"{target_gateway_url.rstrip('/')}/ingest/upload"
                        print(f"[yt-dlp-ingest] Uploading '{title}' to gateway...", flush=True)
                        res = httpx.post(upload_url, files=files, headers=headers, timeout=60.0)
                        if res.status_code >= 400:
                            print(f"[yt-dlp-ingest] upload rejected {res.status_code}: {res.text}", flush=True)
                        res.raise_for_status()
                        print(f"[yt-dlp-ingest] Ingested '{title}' status: {res.status_code}", flush=True)
                finally:
                    if os.path.exists(mp3_filename):
                        os.remove(mp3_filename)

            _report_job(job_id, user_id, target_gateway_url, ok=True)
            return  # Success!

        except Exception as e:
            err_str = str(e)
            permanent = (
                "Video unavailable" in err_str
                or "Private video" in err_str
                or "Sign in to confirm your age" in err_str
            )
            if permanent:
                _report_job(job_id, user_id, target_gateway_url, ok=False, error=err_str)
                return

            if attempt < MAX_RETRIES:
                sleep_time = random.uniform(0, min(120.0, BASE_BACKOFF_SECONDS * (2 ** attempt)))
                print(f"[yt-dlp-ingest] job {job_id} failed on attempt {attempt + 1}/{MAX_RETRIES + 1}: {e}. Retrying in {sleep_time:.2f}s...", flush=True)
                time.sleep(sleep_time)
            else:
                print(f"[yt-dlp-ingest] job {job_id} permanently failed after {MAX_RETRIES + 1} attempts: {e}", flush=True)
                _report_job(job_id, user_id, target_gateway_url, ok=False, error=err_str)
        finally:
            shutil.rmtree(job_tmp, ignore_errors=True)


@app.get("/health")
async def health():
    return {"status": "ok"}


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
                if not url and vid:
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
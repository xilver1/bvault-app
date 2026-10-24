import os
import logging
import sys
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

from opentelemetry import trace
from opentelemetry.exporter.otlp.proto.http.trace_exporter import OTLPSpanExporter
from opentelemetry.instrumentation.fastapi import FastAPIInstrumentor
from opentelemetry.instrumentation.httpx import HTTPXClientInstrumentor
from opentelemetry.sdk.resources import SERVICE_NAME, Resource
from opentelemetry.sdk.trace import TracerProvider
from opentelemetry.sdk.trace.export import BatchSpanProcessor
from opentelemetry.trace import SpanKind, Status, StatusCode
from opentelemetry.trace.propagation.tracecontext import TraceContextTextMapPropagator

# Logs go to stdout, where the Alloy DaemonSet tails the pod log and ships it to
# Loki. Two things are needed for that to be worth anything in a container:
#
#   * A real level on every line. Bare `print` carries none, so LogQL can't
#     filter this service by severity the way it can the Rust ones.
#   * Unbuffered output. Python block-buffers stdout when it isn't a TTY, so a
#     crash otherwise takes the last few KB of logs with it — precisely the
#     lines worth reading. `PYTHONUNBUFFERED=1` is set in the Dockerfile.
#
# uvicorn configures its own loggers and leaves the root logger alone, so this
# stands regardless of import order; its startup/access lines keep uvicorn's
# format and still reach Loki.
logging.basicConfig(
    level=os.getenv("LOG_LEVEL", "INFO").upper(),
    format="%(asctime)s %(levelname)-7s %(name)s: %(message)s",
    datefmt="%Y-%m-%dT%H:%M:%S%z",
    stream=sys.stdout,
)
log = logging.getLogger("yt-dlp-ingest")


class _DropHealthyProbes(logging.Filter):
    """Keep the kubelet readiness probe out of Loki.

    It hits /health every 10s — ~8.6k lines a day that say nothing. Only
    successful probes are dropped; a failing one is exactly what you'd want to
    see, so anything >=400 still gets through.
    """

    def filter(self, record: logging.LogRecord) -> bool:
        # uvicorn logs access lines as
        # ("%s - \"%s %s HTTP/%s\" %d", client, method, path, version, status)
        args = record.args
        if isinstance(args, tuple) and len(args) >= 5:
            path, status = args[2], args[4]
            return not (path == "/health" and isinstance(status, int) and status < 400)
        return True


logging.getLogger("uvicorn.access").addFilter(_DropHealthyProbes())


# yt-dlp writes to stdout itself unless handed a logger. Raw prints reach the
# pod log, so Alloy ships them either way — but they arrive as levelless text
# that can't be filtered or separated from the service's own lines. Routing
# them through `logging` puts a level on each one and files them under their
# own logger name, so LogQL can isolate extractor chatter from ingest events.
_ytdlp_log = log.getChild("yt-dlp")


class _YtDlpLogger:
    """Adapts yt-dlp's logger protocol onto the stdlib logger.

    For youtube-dl compatibility yt-dlp funnels *both* debug and info through
    `debug()`, distinguishing them only by a "[debug] " prefix. Splitting them
    back into real levels has to happen here — otherwise every extractor
    internal lands in Loki at INFO, which is the noise this replaced.
    """

    def debug(self, msg: str) -> None:
        if msg.startswith("[debug] "):
            _ytdlp_log.debug("%s", msg[len("[debug] "):])
        else:
            _ytdlp_log.info("%s", msg)

    def info(self, msg: str) -> None:
        _ytdlp_log.info("%s", msg)

    def warning(self, msg: str) -> None:
        _ytdlp_log.warning("%s", msg)

    def error(self, msg: str) -> None:
        _ytdlp_log.error("%s", msg)


_YTDLP_LOGGER = _YtDlpLogger()


def _init_tracing() -> bool:
    """Export spans to Tempo over OTLP/HTTP, or do nothing at all.

    Deliberately the same contract as the Rust services: with no
    OTEL_EXPORTER_OTLP_ENDPOINT in the environment no SDK is installed, so local
    runs behave exactly as before, and a collector that is misconfigured or down
    costs a warning rather than the ingestion pipeline.

    Endpoint, headers and resource attributes are read from the standard OTEL_*
    environment by the SDK itself, so the generic endpoint variable gets
    /v1/traces appended exactly as it does on the Rust side.
    """
    if os.getenv("OTEL_SDK_DISABLED", "false").lower() == "true":
        return False
    endpoint = os.getenv("OTEL_EXPORTER_OTLP_TRACES_ENDPOINT") or os.getenv(
        "OTEL_EXPORTER_OTLP_ENDPOINT"
    )
    if not endpoint:
        return False
    # A bare host:port is accepted here but fails on every single export
    # ("No connection adapters were found"), so catch it once at startup
    # instead of once per batch, forever.
    if not endpoint.startswith(("http://", "https://")):
        log.error(
            "OTLP endpoint %r has no scheme (expected e.g. http://host:4318); "
            "continuing without traces",
            endpoint,
        )
        return False

    try:
        service = os.getenv("OTEL_SERVICE_NAME") or "bvault-yt-dlp-ingest"
        provider = TracerProvider(resource=Resource.create({SERVICE_NAME: service}))
        provider.add_span_processor(BatchSpanProcessor(OTLPSpanExporter()))
        trace.set_tracer_provider(provider)
        log.info("otlp trace export enabled (service=%s)", service)
        return True
    except Exception:
        log.exception("otlp trace export unavailable; continuing without traces")
        return False


TRACING_ENABLED = _init_tracing()
# Safe either way: with no provider installed this hands back a no-op tracer, so
# the instrumentation below and the job spans need no enabled/disabled branches.
tracer = trace.get_tracer("bvault-yt-dlp-ingest")

app = FastAPI(title="BeatVault yt-dlp Ingestion Service")

if TRACING_ENABLED:
    # Server spans. /health is excluded for the same reason it is filtered out
    # of the logs: the readiness probe every 10s is noise, not traffic.
    FastAPIInstrumentor.instrument_app(app, excluded_urls="health")
    # Client spans for every httpx call back to the gateway — claim, report,
    # cookie sync, upload. Each one pairs with the gateway's server span to draw
    # a yt-dlp-ingest → gateway edge in the service graph.
    HTTPXClientInstrumentor().instrument()

@app.on_event("startup")
async def startup_event():
    # Limit concurrent yt-dlp/ffmpeg jobs to 2 per pod to prevent CPU starvation and OOM kills.
    limiter = to_thread.current_default_thread_limiter()
    limiter.total_tokens = 2

    # Pre-initialize yt-dlp to avoid thread race conditions during plugin registration.
    # Without this, multiple threads calling yt_dlp.YoutubeDL() concurrently will crash
    # with "AssertionError: PoTokenProvider already registered".
    try:
        with yt_dlp.YoutubeDL({'quiet': True, 'no_warnings': True, 'logger': _YTDLP_LOGGER}) as ydl:
            pass
    except Exception:
        log.exception("yt-dlp pre-init failed")

    # Start a single claim loop that manages concurrency internally
    asyncio.create_task(claim_worker_loop())

GATEWAY_URL = os.getenv("GATEWAY_URL", "http://gateway.bvault-prod.svc.cluster.local:8080")
INTERNAL_API_KEY = os.getenv("INTERNAL_API_KEY", "")
POT_PROVIDER_URL = os.getenv("POT_PROVIDER_URL", "http://127.0.0.1:4416")
MAX_RETRIES = int(os.getenv("MAX_RETRIES", "10"))
BASE_BACKOFF_SECONDS = float(os.getenv("BASE_BACKOFF_SECONDS", "10.0"))

async def claim_worker_loop():
    log.info("worker loop started")
    claim_url = f"{GATEWAY_URL.rstrip('/')}/internal/jobs/claim"
    headers = {}
    if INTERNAL_API_KEY:
        headers["X-Internal-Key"] = INTERNAL_API_KEY

    # Limit to 2 total active jobs running
    semaphore = asyncio.Semaphore(2)

    async def handle_job(job):
        url = job["payload"]["url"]
        user_id = job["payload"]["user_id"]
        job_id = job["id"]

        # Join the trace of whoever asked for this ingest. The gateway stored its
        # traceparent on the job row and hands it back on claim; adopting it as a
        # remote parent makes this span the *consumer* half of the queue's
        # service-graph edge, rather than a trace that appears from nowhere
        # minutes after the request that caused it returned.
        traceparent = job.get("traceparent")
        parent = (
            TraceContextTextMapPropagator().extract({"traceparent": traceparent})
            if traceparent
            else None
        )

        with tracer.start_as_current_span(
            "ytdlp-ingest-job",
            context=parent,
            # Must mirror the producer side in the gateway's Queue::enqueue, or
            # Tempo has nothing to pair this with.
            kind=SpanKind.CONSUMER,
            attributes={
                "messaging.system": "postgresql",
                "messaging.operation.name": "process",
                "messaging.destination.name": "yt_dlp_ingest",
                "job.id": job_id,
            },
        ) as span:
            try:
                for attempt in range(MAX_RETRIES + 1):
                    span.set_attribute("job.attempt", attempt + 1)
                    # The thread limiter ensures only 2 of these actually run concurrently
                    success, permanent, err_str = await to_thread.run_sync(process_yt_dlp_once, url, user_id, job_id, GATEWAY_URL)

                    if success:
                        _report_job(job_id, user_id, GATEWAY_URL, ok=True)
                        return

                    if permanent:
                        span.set_status(Status(StatusCode.ERROR, err_str))
                        _report_job(job_id, user_id, GATEWAY_URL, ok=False, error=err_str)
                        return

                    if attempt < MAX_RETRIES:
                        sleep_time = random.uniform(0, min(120.0, BASE_BACKOFF_SECONDS * (2 ** attempt)))
                        log.warning(
                            "job %s failed on attempt %d/%d: %s — retrying in %.2fs",
                            job_id, attempt + 1, MAX_RETRIES + 1, err_str, sleep_time,
                        )
                        await asyncio.sleep(sleep_time)

                span.set_status(Status(StatusCode.ERROR, err_str))
                log.error(
                    "job %s permanently failed after %d attempts: %s",
                    job_id, MAX_RETRIES + 1, err_str,
                )
                _report_job(job_id, user_id, GATEWAY_URL, ok=False, error=err_str)
            finally:
                semaphore.release()

    async with httpx.AsyncClient(timeout=30.0) as client:
        while True:
            try:
                await semaphore.acquire()
                res = await client.post(claim_url, json={"kind": "yt_dlp_ingest", "lease_secs": 1800}, headers=headers)
                if res.status_code == 204:
                    semaphore.release()
                    await asyncio.sleep(5)
                    continue

                res.raise_for_status()
                job = res.json()
                
                # Spawn a background task for the job so the claim loop can immediately fetch the next one
                asyncio.create_task(handle_job(job))
                
            except httpx.HTTPError as e:
                log.warning("http error claiming job: %s", e)
                semaphore.release()
                await asyncio.sleep(5)
            except Exception:
                log.exception("unexpected error in worker loop")
                semaphore.release()
                await asyncio.sleep(5)


def _report_job(job_id: int, user_id: str, gateway_url: str, ok: bool, error: Optional[str] = None):
    """Flip the job's terminal state on the gateway (which owns the DB)."""
    try:
        url = f"{gateway_url.rstrip('/')}/internal/jobs/{job_id}"
        headers = {"X-User-Id": user_id}
        if INTERNAL_API_KEY:
            headers["X-Internal-Key"] = INTERNAL_API_KEY
        httpx.post(url, json={"ok": ok, "error": error}, headers=headers, timeout=30.0)
    except Exception:
        log.exception("failed to report job %s", job_id)


def process_yt_dlp_once(url: str, user_id: str, job_id: int, target_gateway_url: str):
    job_tmp = os.path.join(tempfile.gettempdir(), f"ytdlp-{job_id}-{uuid.uuid4().hex[:8]}")
    os.makedirs(job_tmp, exist_ok=True)
    
    client_mixes = [
        ['web_embedded', 'default', 'mweb', 'web'],
        ['mweb', 'web_embedded', 'default', 'android'],
        ['default', 'web_embedded', 'web', 'mweb'],
        ['android', 'ios', 'web_embedded', 'default']
    ]
    chosen_clients = random.choice(client_mixes)
    
    try:
        cookie_file = os.path.join(job_tmp, "cookies.txt")
        headers = {"X-User-Id": user_id}
        if INTERNAL_API_KEY:
            headers["X-Internal-Key"] = INTERNAL_API_KEY

        try:
            cookie_res = httpx.get(f"{target_gateway_url.rstrip('/')}/auth/youtube/cookies", headers=headers, timeout=10.0)
            if cookie_res.status_code == 200:
                cookies_txt = cookie_res.json().get("cookies_txt", "")
                if cookies_txt:
                    with open(cookie_file, "w") as f:
                        f.write(cookies_txt)
                    log.info("loaded cookies for user %s", user_id)
        except Exception as e:
            log.warning("failed to fetch cookies: %s", e)

        ydl_opts = {
            'force_ipv4': True,
            'format': 'bestaudio/best',
            'outtmpl': '%(id)s.%(ext)s',
            'paths': {'home': job_tmp, 'temp': job_tmp},
            'cookiefile': cookie_file,
            'postprocessors': [{
                'key': 'FFmpegExtractAudio',
                'preferredcodec': 'mp3',
                'preferredquality': '320',
            }],
            # `quiet`/`no_warnings` stay off so extraction and destination
            # lines still reach the logger; `verbose` is not set, since its
            # [debug] firehose was the bulk of this pod's log volume (it is
            # still reachable per-pod with LOG_LEVEL=DEBUG).
            'quiet': False,
            'no_warnings': False,
            'logger': _YTDLP_LOGGER,
            # Progress ticks are redrawn continuously; through a logger each
            # redraw becomes its own line, so a single download would be
            # thousands of them.
            'noprogress': True,
            'impersonate': ImpersonateTarget.from_str('chrome'),
                # Fetch a GVS PO token from the bgutil sidecar so YouTube authorizes
                # the media download (this is the fix for the 403). base_url matches
                # the plugin default, set explicitly so it's greppable + overridable.
            'extractor_args': {
                'youtube': {'player_client': chosen_clients},
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
                    log.info("uploading %r to gateway", title)
                    res = httpx.post(upload_url, files=files, headers=headers, timeout=60.0)
                    if res.status_code >= 400:
                        log.error("upload rejected %s: %s", res.status_code, res.text)
                    res.raise_for_status()
                    log.info("ingested %r, gateway returned %s", title, res.status_code)
            finally:
                if os.path.exists(mp3_filename):
                    os.remove(mp3_filename)

        # Upload refreshed cookies back to gateway
        if os.path.exists(cookie_file):
            try:
                with open(cookie_file, "r") as f:
                    new_cookies = f.read()
                httpx.post(f"{target_gateway_url.rstrip('/')}/auth/youtube/cookies", json={"cookies_txt": new_cookies}, headers=headers, timeout=10.0)
                log.info("updated cookies for user %s", user_id)
            except Exception as e:
                log.warning("failed to update cookies: %s", e)

        return (True, False, "")

    except Exception as e:
        err_str = str(e)
        permanent = (
            "Video unavailable" in err_str
            or "Private video" in err_str
            or "Sign in to confirm your age" in err_str
            or "Sign in to confirm you're not a bot" in err_str
        )
        return (False, permanent, err_str)

    finally:
        shutil.rmtree(job_tmp, ignore_errors=True)


@app.get("/health")
async def health():
    return {"status": "ok"}


@app.get("/search")
def search_yt_dlp(q: str, limit: int = 10, x_internal_key: Optional[str] = Header(None)):
    if INTERNAL_API_KEY and x_internal_key != INTERNAL_API_KEY:
        raise HTTPException(status_code=401, detail="Invalid internal key")

    ydl_opts = {"extract_flat": True, "quiet": True, "no_warnings": True, "logger": _YTDLP_LOGGER}
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
        except Exception:
            log.exception("search error")
            raise HTTPException(status_code=500, detail="Search failed")
    return results
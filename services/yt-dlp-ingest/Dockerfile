FROM python:3.11-slim

# External JS runtime for YouTube nsig / challenge solving. This is separate
# from the PO token (which comes from the sidecar); it's belt-and-suspenders for
# the signature path that 2026 YouTube increasingly gates. Drop it if you want a
# leaner image and your videos don't need it.
COPY --from=denoland/deno:bin /deno /usr/local/bin/deno

RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    ca-certificates \
    build-essential \
    libffi-dev \
    libssl-dev \
    libnss3 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY services/yt-dlp-ingest/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY services/yt-dlp-ingest/app.py .

EXPOSE 8080

CMD ["uvicorn", "app:app", "--host", "0.0.0.0", "--port", "8080"]
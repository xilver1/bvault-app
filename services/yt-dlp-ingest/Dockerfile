FROM python:3.11-slim

RUN apt-get update && apt-get install -y --no-install-recommends \
    ffmpeg \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

COPY services/yt-dlp-ingest/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY services/yt-dlp-ingest/app.py .

EXPOSE 8080

CMD ["uvicorn", "app.py:app", "--host", "0.0.0.0", "--port", "8080"]
